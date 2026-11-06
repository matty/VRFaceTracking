use crate::osc::native_face::{self, NativeFaceSender};
use crate::osc::parameters::registry::ParameterRegistry;
use crate::osc::parameters::ParamType;
use crate::osc::query::service::{AvatarInfo, OscParamType, OscQueryService};
use crate::osc::query::target::VrchatTarget;
use anyhow::Result;
use log::{error, info, warn};
use rosc::{decoder, encoder, OscPacket, OscType};
use std::collections::{HashMap, HashSet};
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use vrft_common::UnifiedTrackingData;

/// The eye endpoints the native face blob takes over from: it carries gaze and
/// both lids itself, and wins over these on an avatar that takes it.
const TRACKING_EYE_PREFIX: &str = "/tracking/eye/";

pub struct VRChatOsc {
    socket: Mutex<Option<UdpSocket>>,
    target: VrchatTarget,
    /// The socket VRChat's replies arrive on, until the listener takes it.
    recv_socket: Mutex<Option<UdpSocket>>,
    receive_port: u16,
    osc_query_service: Mutex<Option<OscQueryService>>,
    query_rx: Mutex<Receiver<Option<AvatarInfo>>>,
    change_tx_query: Sender<String>,
    pub param_registry: Mutex<ParameterRegistry>,
    native_face: Mutex<NativeFaceSender>,
    shutdown_flag: Arc<AtomicBool>,
}

impl VRChatOsc {
    /// Listens for VRChat's replies on `receive_port`, or on any free port
    /// when that's taken; see [`Self::receive_port`].
    pub fn new(target: VrchatTarget, receive_port: u16) -> Self {
        let recv_socket = bind_receive(receive_port);
        let receive_port = recv_socket
            .as_ref()
            .and_then(|socket| socket.local_addr().ok())
            .map_or(receive_port, |addr| addr.port());
        let (query_tx, query_rx) = channel();
        let (change_tx_query, change_rx_query) = channel();

        let osc_query_service = OscQueryService::new(query_tx, change_rx_query, target.clone());

        Self {
            socket: Mutex::new(None),
            target,
            recv_socket: Mutex::new(recv_socket),
            receive_port,
            osc_query_service: Mutex::new(Some(osc_query_service)),
            query_rx: Mutex::new(query_rx),
            change_tx_query,
            param_registry: Mutex::new(ParameterRegistry::new()),
            native_face: Mutex::new(NativeFaceSender::default()),
            shutdown_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The port VRChat's replies arrive on, to advertise over OSCQuery.
    pub fn receive_port(&self) -> u16 {
        self.receive_port
    }

    pub fn initialize(&mut self) -> Result<()> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;

        if let Ok(mut guard) = self.osc_query_service.lock() {
            if let Some(service) = guard.take() {
                if let Err(e) = service.start() {
                    error!("Failed to start OSC Query Service: {}", e);
                } else {
                    info!("OSC Query Service started (managed by VRChatOsc)");
                }
            }
        }

        let recv_socket = self
            .recv_socket
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| anyhow::anyhow!("No port free to listen for VRChat on"))?;
        // Set socket timeout for graceful shutdown (500ms)
        recv_socket.set_read_timeout(Some(Duration::from_millis(500)))?;

        let tx_query = self.change_tx_query.clone();
        let port = self.receive_port;
        let shutdown = self.shutdown_flag.clone();
        let target = self.target.clone();

        let listen = move || {
            info!("Listening for OSC messages on port {}", port);
            // Max UDP payload. A datagram larger than the buffer fails with
            // WSAEMSGSIZE on Windows and is discarded rather than truncated.
            let mut buf = [0u8; 65535];
            while !shutdown.load(Ordering::Relaxed) {
                match recv_socket.recv_from(&mut buf) {
                    Ok((size, _addr)) => {
                        if let Ok((_, packet)) = decoder::decode_udp(&buf[..size]) {
                            handle_packet(packet, &tx_query, &target);
                        }
                    }
                    Err(ref e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        // Timeout - check shutdown flag and continue
                        continue;
                    }
                    Err(e) => {
                        error!("Error receiving OSC packet: {}", e);
                    }
                }
            }
            info!("OSC listener thread exiting gracefully");
        };
        thread::Builder::new()
            .name("osc-listen".into())
            .spawn(listen)?;

        *self.socket.lock().unwrap() = Some(socket);
        Ok(())
    }

    pub fn send(&self, data: &UnifiedTrackingData) -> Result<()> {
        // Check for parameter updates (quick lock)
        if let Ok(rx) = self.query_rx.lock() {
            while let Ok(update) = rx.try_recv() {
                // Convert Vec<OscParameterInfo> to the structures we need
                if let Some(avatar) = &update {
                    if let Some(native) = avatar.full_face_tracking {
                        self.target.offer_avatar_native_face_tracking(native);
                    }
                    let param_infos = &avatar.parameters;
                    let avatar_params: HashSet<String> =
                        param_infos.iter().map(|p| p.address.clone()).collect();

                    let param_types: HashMap<String, ParamType> = param_infos
                        .iter()
                        .map(|p| {
                            let param_type = match p.param_type {
                                OscParamType::Bool => ParamType::Bool,
                                OscParamType::Int => ParamType::Int,
                                OscParamType::Float | OscParamType::Unknown => ParamType::Float,
                            };
                            (p.address.clone(), param_type)
                        })
                        .collect();

                    let ft_sample: Vec<_> = avatar_params
                        .iter()
                        .filter(|p| p.contains("FT") || p.contains("v2"))
                        .take(10)
                        .collect();
                    log::info!("FT/v2 sample params: {:?}", ft_sample);

                    // Reset parameter registry with real types from OSC Query
                    if let Ok(mut registry) = self.param_registry.lock() {
                        log::info!("Calling registry.reset()...");
                        let relevant = registry.reset(&avatar_params, &param_types);
                        log::info!("registry.reset() completed");
                        self.target.set_avatar_face_tracking(Some(relevant > 0));
                    } else {
                        log::error!("Failed to acquire param_registry lock!");
                    }
                }
            }
        }

        let mut messages = {
            let mut registry = self.param_registry.lock().unwrap();
            registry.process(data)
        };

        let native = self.target.native_face_tracking();
        if native {
            messages.retain(|message| !message.addr.starts_with(TRACKING_EYE_PREFIX));
        }
        let native_face = self
            .native_face
            .lock()
            .unwrap()
            .next(data, native, Instant::now());

        if messages.is_empty() && native_face.is_none() {
            return Ok(());
        }

        let mut datagrams = super::encode_bundles(messages)?;
        // In a datagram of its own: the one form VRChat's spec describes.
        if let Some(message) = native_face {
            datagrams.push(encoder::encode(&OscPacket::Message(message))?);
        }

        // Send to target address
        let mut socket_guard = self.socket.lock().unwrap();

        if socket_guard.is_none() {
            match UdpSocket::bind("0.0.0.0:0") {
                Ok(s) => {
                    info!("Re-bound OSC socket successfully.");
                    *socket_guard = Some(s);
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("Failed to re-bind OSC socket: {}", e));
                }
            }
        }

        let socket = socket_guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("VRChatOsc socket not available"))?;

        let address = self.target.send_addr();
        for datagram in datagrams {
            if let Err(e) = socket.send_to(&datagram, &address) {
                // The caller logs it; the socket is made again next frame.
                *socket_guard = None;
                return Err(anyhow::anyhow!("OSC Send failed: {}", e));
            }
        }
        Ok(())
    }

    /// Leaves a native avatar's face neutral, because VRChat documents no
    /// timeout that would release the last blob when we stop sending.
    pub fn send_neutral_face(&self) {
        if !self.target.native_face_tracking() {
            return;
        }
        let packet = OscPacket::Message(native_face::neutral_message());
        let sent = encoder::encode(&packet)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| {
                let socket = UdpSocket::bind("0.0.0.0:0")?;
                socket.send_to(&bytes, self.target.send_addr())?;
                Ok(())
            });
        if let Err(e) = sent {
            warn!("Failed to send the neutral face: {}", e);
        }
    }

    /// Signal the OSC listener thread to shut down gracefully
    pub fn shutdown(&self) {
        info!("Shutting down OSC listener...");
        self.shutdown_flag.store(true, Ordering::Relaxed);
    }
}

/// Binds the socket VRChat's replies arrive on: `port`, or any free port when
/// another app has it.
fn bind_receive(port: u16) -> Option<UdpSocket> {
    let socket = UdpSocket::bind(("0.0.0.0", port))
        .or_else(|e| {
            warn!(
                "Port {} is in use ({}); listening for VRChat on a free port instead",
                port, e
            );
            UdpSocket::bind("0.0.0.0:0")
        })
        .inspect_err(|e| error!("Failed to listen for VRChat: {}", e))
        .ok()?;
    // Set socket timeout for graceful shutdown (500ms)
    if let Err(e) = socket.set_read_timeout(Some(Duration::from_millis(500))) {
        error!("Failed to set the VRChat listener's timeout: {}", e);
        return None;
    }
    Some(socket)
}

fn handle_packet(packet: OscPacket, tx_query: &Sender<String>, target: &VrchatTarget) {
    match packet {
        OscPacket::Message(msg) => {
            if msg.addr == "/avatar/change" {
                let avatar_id = if let Some(arg) = msg.args.first() {
                    match arg {
                        OscType::String(s) => s.clone(),
                        _ => "Unknown".to_string(),
                    }
                } else {
                    "Unknown".to_string()
                };
                info!("Avatar change detected! New Avatar ID: {}", avatar_id);
                // Not known for the new avatar until VRChat says.
                target.avatar_changed();
                let _ = tx_query.send(avatar_id);
            } else if msg.addr == native_face::AVATAR_FULL_FACE_TRACKING {
                if let Some(OscType::Bool(native)) = msg.args.first() {
                    info!("Avatar takes native face tracking: {}", native);
                    target.set_avatar_native_face_tracking(*native);
                }
            }
        }
        OscPacket::Bundle(bundle) => {
            for packet in bundle.content {
                handle_packet(packet, tx_query, target);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rosc::OscMessage;

    #[test]
    fn a_taken_receive_port_falls_back_to_a_free_one() {
        let taken = UdpSocket::bind("0.0.0.0:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        let socket = bind_receive(port).expect("a free port");
        assert_ne!(socket.local_addr().unwrap().port(), port);
    }

    #[test]
    fn the_advertised_port_is_the_one_bound() {
        let taken = UdpSocket::bind("0.0.0.0:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        let osc = VRChatOsc::new(VrchatTarget::new("127.0.0.1", 9000), port);
        assert_ne!(osc.receive_port(), port);
        assert_ne!(osc.receive_port(), 0);
    }

    #[test]
    fn an_avatar_change_forgets_whether_the_avatar_had_face_tracking() {
        let target = VrchatTarget::new("127.0.0.1", 9000);
        target.found(Some(9000));
        target.set_avatar_face_tracking(Some(true));
        let (tx_query, rx_query) = channel();
        let change = OscPacket::Message(OscMessage {
            addr: "/avatar/change".into(),
            args: vec![OscType::String("avtr_test".into())],
        });

        handle_packet(change, &tx_query, &target);

        assert_eq!(target.link().avatar_face_tracking, None);
        assert_eq!(rx_query.try_recv().unwrap(), "avtr_test");
    }

    fn full_face_tracking(value: OscType) -> OscPacket {
        OscPacket::Message(OscMessage {
            addr: "/avatar/avatarfullfacetracking".into(),
            args: vec![value],
        })
    }

    #[test]
    fn vrchat_saying_so_turns_native_face_tracking_on_and_off() {
        let target = VrchatTarget::new("127.0.0.1", 9000);
        let (tx_query, _rx_query) = channel();

        handle_packet(full_face_tracking(OscType::Bool(true)), &tx_query, &target);
        assert!(target.native_face_tracking());

        handle_packet(full_face_tracking(OscType::Bool(false)), &tx_query, &target);
        assert!(!target.native_face_tracking());
    }

    #[test]
    fn an_unexpected_argument_changes_nothing() {
        let target = VrchatTarget::new("127.0.0.1", 9000);
        let (tx_query, _rx_query) = channel();
        handle_packet(full_face_tracking(OscType::Bool(true)), &tx_query, &target);
        handle_packet(full_face_tracking(OscType::Int(0)), &tx_query, &target);
        assert!(target.native_face_tracking());
    }

    /// Everything arriving at a stand-in for VRChat from one `send`, unpacked
    /// from bundles, and whether each came on its own rather than in a bundle.
    fn sent_by(osc: &VRChatOsc, listener: &UdpSocket) -> Vec<(OscMessage, bool)> {
        osc.send(&UnifiedTrackingData::default()).unwrap();
        listener
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        let mut buf = [0u8; 65535];
        let mut received = Vec::new();
        while let Ok(size) = listener.recv(&mut buf) {
            match decoder::decode_udp(&buf[..size]).unwrap().1 {
                OscPacket::Message(message) => received.push((message, true)),
                OscPacket::Bundle(bundle) => {
                    received.extend(bundle.content.into_iter().map(|packet| match packet {
                        OscPacket::Message(message) => (message, false),
                        OscPacket::Bundle(_) => panic!("nested bundle"),
                    }))
                }
            }
        }
        received
    }

    /// A VRChatOsc sending to a local listener, for an avatar with `params`.
    fn osc_for_avatar(params: &[&str]) -> (VRChatOsc, UdpSocket) {
        let listener = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let osc = VRChatOsc::new(VrchatTarget::new("127.0.0.1", port), 0);
        let avatar_params: HashSet<String> = params
            .iter()
            .map(|p| format!("/avatar/parameters/{p}"))
            .collect();
        let types = avatar_params
            .iter()
            .map(|p| (p.clone(), ParamType::Float))
            .collect();
        osc.param_registry
            .lock()
            .unwrap()
            .reset(&avatar_params, &types);
        (osc, listener)
    }

    #[test]
    fn a_native_avatar_gets_the_blob_on_its_own_instead_of_the_eye_endpoints() {
        let (osc, listener) = osc_for_avatar(&["SomethingElse"]);
        osc.target.set_avatar_native_face_tracking(true);

        let sent = sent_by(&osc, &listener);

        let blobs: Vec<_> = sent
            .iter()
            .filter(|(m, _)| m.addr == native_face::ADDRESS)
            .collect();
        let [(message, alone)] = blobs.as_slice() else {
            panic!("expected one blob, got {sent:?}")
        };
        assert!(*alone, "the blob should not be in a bundle");
        let [OscType::Blob(payload)] = message.args.as_slice() else {
            panic!("expected one blob argument, got {:?}", message.args)
        };
        assert_eq!(payload.len(), native_face::PAYLOAD_LEN);
        assert!(
            !sent
                .iter()
                .any(|(m, _)| m.addr.starts_with("/tracking/eye/")),
            "{sent:?}"
        );
    }

    /// The blob is the only output for an avatar with no parameters we drive,
    /// so an empty parameter list must not end the frame early.
    #[test]
    fn a_frame_with_no_parameters_still_sends_the_blob() {
        let (osc, listener) = osc_for_avatar(&[]);
        osc.target.set_avatar_native_face_tracking(true);

        let sent = sent_by(&osc, &listener);

        let [(message, true)] = sent.as_slice() else {
            panic!("expected only the blob, got {sent:?}")
        };
        assert_eq!(message.addr, native_face::ADDRESS);
    }

    #[test]
    fn stopping_leaves_a_native_avatar_neutral() {
        let (osc, listener) = osc_for_avatar(&[]);
        osc.target.set_avatar_native_face_tracking(true);

        osc.send_neutral_face();

        let mut buf = [0u8; 65535];
        listener
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        let size = listener.recv(&mut buf).unwrap();
        let OscPacket::Message(message) = decoder::decode_udp(&buf[..size]).unwrap().1 else {
            panic!("expected a message")
        };
        assert_eq!(message.addr, native_face::ADDRESS);
        assert_eq!(
            message.args,
            vec![OscType::Blob(vec![0; native_face::PAYLOAD_LEN])]
        );
    }

    #[test]
    fn stopping_sends_nothing_to_an_older_avatar() {
        let (osc, listener) = osc_for_avatar(&["JawOpen"]);

        osc.send_neutral_face();

        listener
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        assert!(listener.recv(&mut [0u8; 2048]).is_err());
    }

    #[test]
    fn an_older_avatar_keeps_the_eye_endpoints_and_gets_no_blob() {
        let (osc, listener) = osc_for_avatar(&["JawOpen"]);

        let sent = sent_by(&osc, &listener);

        assert!(!sent.iter().any(|(m, _)| m.addr == native_face::ADDRESS));
        assert!(sent
            .iter()
            .any(|(m, _)| m.addr.starts_with("/tracking/eye/")));
    }

    /// An avatar can carry both; parameters may drive things the native
    /// section does not, so they keep flowing.
    #[test]
    fn a_native_avatar_with_old_parameters_keeps_getting_them() {
        let (osc, listener) = osc_for_avatar(&["v2/JawOpen"]);
        osc.target.set_avatar_native_face_tracking(true);

        let sent = sent_by(&osc, &listener);

        assert!(sent.iter().any(|(m, _)| m.addr == native_face::ADDRESS));
        assert!(sent
            .iter()
            .any(|(m, _)| m.addr == "/avatar/parameters/v2/JawOpen"));
    }
}
