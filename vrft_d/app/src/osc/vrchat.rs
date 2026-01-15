use crate::osc::query::service::OscQueryService;
use crate::parameter_solver::ParameterSolver;
use anyhow::Result;
use common::UnifiedTrackingData;
use log::{error, info};
use rosc::{decoder, encoder, OscBundle, OscMessage, OscPacket, OscType};
use std::collections::HashSet;
use std::net::UdpSocket;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;
use std::thread;

pub struct VRChatOsc {
    socket: Mutex<Option<UdpSocket>>,
    target_addr: String,
    receive_port: u16,
    allowed_parameters: Mutex<Option<HashSet<String>>>,
    osc_query_service: Mutex<Option<OscQueryService>>,
    query_rx: Mutex<Receiver<Option<HashSet<String>>>>,
    change_tx_calibration: Sender<String>,
    change_tx_query: Sender<String>,
    pub change_rx: Mutex<Option<Receiver<String>>>,
    pub parameter_buffer: Mutex<Vec<(&'static str, f32)>>,
}

impl VRChatOsc {
    pub fn new(target_addr: &str, receive_port: u16) -> Self {
        let (query_tx, query_rx) = channel();
        let (change_tx_calibration, change_rx_calibration) = channel();
        let (change_tx_query, change_rx_query) = channel();

        let osc_query_service = OscQueryService::new(query_tx, change_rx_query);

        Self {
            socket: Mutex::new(None),
            target_addr: target_addr.to_string(),
            receive_port,
            allowed_parameters: Mutex::new(None),
            osc_query_service: Mutex::new(Some(osc_query_service)),
            query_rx: Mutex::new(query_rx),
            change_tx_calibration,
            change_tx_query,
            change_rx: Mutex::new(Some(change_rx_calibration)),
            parameter_buffer: Mutex::new(Vec::with_capacity(200)),
        }
    }

    fn is_allowed(&self, addr: &str, allowed_params: &Option<HashSet<String>>) -> bool {
        match allowed_params {
            Some(allowed) => allowed.contains(addr),
            None => true,
        }
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

        let recv_socket = UdpSocket::bind(format!("0.0.0.0:{}", self.receive_port))?;
        let tx_calib = self.change_tx_calibration.clone();
        let tx_query = self.change_tx_query.clone();
        let port = self.receive_port;

        thread::spawn(move || {
            info!("Listening for OSC messages on port {}", port);
            let mut buf = [0u8; 2048];
            loop {
                match recv_socket.recv_from(&mut buf) {
                    Ok((size, _addr)) => {
                        if let Ok((_, packet)) = decoder::decode_udp(&buf[..size]) {
                            handle_packet(packet, &tx_calib, &tx_query);
                        }
                    }
                    Err(e) => {
                        error!("Error receiving OSC packet: {}", e);
                    }
                }
            }
        });

        *self.socket.lock().unwrap() = Some(socket);
        Ok(())
    }

    pub fn send(&self, data: &UnifiedTrackingData) -> Result<()> {
        if let Ok(rx) = self.query_rx.lock() {
            while let Ok(update) = rx.try_recv() {
                if let Ok(mut params) = self.allowed_parameters.lock() {
                    *params = update;
                }
            }
        }

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

        let mut messages = Vec::with_capacity(100);

        let allowed_guard = self.allowed_parameters.lock().unwrap();
        let allowed_ref = &*allowed_guard;

        macro_rules! add_msg {
            ($addr:expr, $val:expr) => {
                let addr_str = $addr.to_string();
                if self.is_allowed(&addr_str, allowed_ref) {
                    messages.push(OscMessage {
                        addr: addr_str,
                        args: vec![OscType::Float($val)],
                    });
                }
            };
        }

        add_msg!("/avatar/parameters/FT/v2/EyeLeftX", -data.eye.left.gaze.x);
        add_msg!("/avatar/parameters/FT/v2/EyeLeftY", -data.eye.left.gaze.y);
        add_msg!("/avatar/parameters/FT/v2/EyeRightX", -data.eye.right.gaze.x);
        add_msg!("/avatar/parameters/FT/v2/EyeRightY", -data.eye.right.gaze.y);
        add_msg!(
            "/avatar/parameters/FT/v2/EyeX",
            -(data.eye.left.gaze.x + data.eye.right.gaze.x) / 2.0
        );
        add_msg!(
            "/avatar/parameters/FT/v2/EyeY",
            -(data.eye.left.gaze.y + data.eye.right.gaze.y) / 2.0
        );

        let lrpy_addr = "/tracking/eye/LeftRightPitchYaw";
        if self.is_allowed(lrpy_addr, allowed_ref) {
            messages.push(OscMessage {
                addr: lrpy_addr.to_string(),
                args: vec![
                    OscType::Float(-data.eye.left.gaze.y.asin()),
                    OscType::Float(-data.eye.left.gaze.x.atan2(data.eye.left.gaze.z)),
                    OscType::Float(-data.eye.right.gaze.y.asin()),
                    OscType::Float(-data.eye.right.gaze.x.atan2(data.eye.right.gaze.z)),
                ],
            });
        }

        if let Ok(mut buffer) = self.parameter_buffer.lock() {
            ParameterSolver::solve(data, &mut buffer);
            for (name, value) in buffer.iter() {
                let addr = format!("/avatar/parameters/FT/{}", name);
                if self.is_allowed(&addr, allowed_ref) {
                    messages.push(OscMessage {
                        addr,
                        args: vec![OscType::Float(*value)],
                    });
                }
            }
        }

        drop(allowed_guard);

        if messages.is_empty() {
            return Ok(());
        }

        let bundle = OscBundle {
            timetag: rosc::OscTime::from((0, 0)),
            content: messages.into_iter().map(OscPacket::Message).collect(),
        };

        let packet = OscPacket::Bundle(bundle);
        let msg_buf = encoder::encode(&packet)?;

        match socket.send_to(&msg_buf, &self.target_addr) {
            Ok(_) => Ok(()),
            Err(e) => {
                error!(
                    "Failed to send OSC packet: {}. Attempting to reconnect...",
                    e
                );
                *socket_guard = None;
                Err(anyhow::anyhow!("OSC Send failed: {}", e))
            }
        }
    }
}

fn handle_packet(packet: OscPacket, tx_calib: &Sender<String>, tx_query: &Sender<String>) {
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
                let _ = tx_calib.send(avatar_id.clone());
                let _ = tx_query.send(avatar_id);
            }
        }
        OscPacket::Bundle(bundle) => {
            for packet in bundle.content {
                handle_packet(packet, tx_calib, tx_query);
            }
        }
    }
}
