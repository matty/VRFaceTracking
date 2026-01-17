use crate::osc::query::service::OscQueryService;
use crate::parameter_solver::ParameterSolver;
use anyhow::Result;
use common::UnifiedTrackingData;
use log::{error, info};
use rosc::{decoder, encoder, OscBundle, OscMessage, OscPacket, OscType};
use std::collections::{HashMap, HashSet};
use std::net::UdpSocket;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
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
        // Check for parameter updates (quick lock)
        if let Ok(rx) = self.query_rx.lock() {
            while let Ok(update) = rx.try_recv() {
                if let Ok(mut params) = self.allowed_parameters.lock() {
                    *params = update;
                }
            }
        }

        // Clone allowed parameters to release lock quickly
        let allowed_params = self.allowed_parameters.lock().unwrap().clone();

        // Build messages without holding any locks
        let mut messages = Vec::with_capacity(100);

        macro_rules! add_msg {
            ($addr:expr, $val:expr) => {
                let addr_str: &str = $addr;
                if is_allowed_static(addr_str, &allowed_params) {
                    messages.push(OscMessage {
                        addr: addr_str.to_string(),
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
        if is_allowed_static(lrpy_addr, &allowed_params) {
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
                let addr = get_cached_osc_address(name);
                if is_allowed_static(addr, &allowed_params) {
                    messages.push(OscMessage {
                        addr: addr.to_string(),
                        args: vec![OscType::Float(*value)],
                    });
                }
            }
        }

        if messages.is_empty() {
            return Ok(());
        }

        // Encode the bundle before acquiring socket lock
        let bundle = OscBundle {
            timetag: rosc::OscTime::from((0, 0)),
            content: messages.into_iter().map(OscPacket::Message).collect(),
        };
        let packet = OscPacket::Bundle(bundle);
        let msg_buf = encoder::encode(&packet)?;

        // Now acquire socket lock only for sending
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

fn is_allowed_static(addr: &str, allowed_params: &Option<HashSet<String>>) -> bool {
    match allowed_params {
        Some(allowed) => allowed.contains(addr),
        None => true,
    }
}

/// Cache for OSC address strings to avoid per-frame allocations
fn get_cached_osc_address(name: &'static str) -> &'static str {
    static ADDRESS_CACHE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();

    let cache = ADDRESS_CACHE.get_or_init(|| {
        let mut map = HashMap::with_capacity(400);

        // Pre-populate with all v2 expression names
        for i in 0..api::UnifiedExpressions::Max as usize {
            if let Some(expr_name) = ParameterSolver::get_expression_name(i) {
                let addr = format!("/avatar/parameters/FT/{}", expr_name);
                map.insert(expr_name, Box::leak(addr.into_boxed_str()) as &'static str);
            }
        }

        // Pre-populate with all legacy parameter names from shape_legacy
        for name in crate::shape_legacy::get_all_parameter_names() {
            if !map.contains_key(name) {
                let addr = format!("/avatar/parameters/FT/{}", name);
                map.insert(name, Box::leak(addr.into_boxed_str()) as &'static str);
            }
        }

        // Pre-populate with all v2 computed parameters from ParameterSolver::solve()
        let v2_computed_params: &[&'static str] = &[
            // Head tracking
            "v2/Head/Yaw",
            "v2/Head/Pitch",
            "v2/Head/Roll",
            "v2/Head/PosX",
            "v2/Head/PosY",
            "v2/Head/PosZ",
            // Brow computed
            "v2/BrowUpRight",
            "v2/BrowUpLeft",
            "v2/BrowDownRight",
            "v2/BrowDownLeft",
            "v2/BrowUp",
            "v2/BrowDown",
            "v2/BrowInnerUp",
            "v2/BrowOuterUp",
            "v2/BrowExpressionRight",
            "v2/BrowExpressionLeft",
            "v2/BrowExpression",
            // Mouth smile/sad
            "v2/MouthSmileRight",
            "v2/MouthSmileLeft",
            "v2/MouthSadRight",
            "v2/MouthSadLeft",
            // Eye computed
            "v2/EyesClosedAmount",
            "v2/PupilDiameterLeft",
            "v2/PupilDiameterRight",
            "v2/PupilDiameter",
            "v2/PupilDilation",
            "v2/EyeOpenLeft",
            "v2/EyeOpenRight",
            "v2/EyeOpen",
            "v2/EyeClosedLeft",
            "v2/EyeClosedRight",
            "v2/EyeClosed",
            "v2/EyeWide",
            "v2/EyeLidLeft",
            "v2/EyeLidRight",
            "v2/EyeLid",
            "v2/EyeSquint",
            "v2/EyesSquint",
            // Jaw computed
            "v2/JawX",
            "v2/JawZ",
            // Cheek computed
            "v2/CheekSquint",
            "v2/CheekPuffSuckLeft",
            "v2/CheekPuffSuckRight",
            "v2/CheekPuffSuck",
            "v2/CheekSuck",
            // Mouth computed
            "v2/MouthUpperX",
            "v2/MouthLowerX",
            "v2/MouthX",
            "v2/LipSuckUpper",
            "v2/LipSuckLower",
            "v2/LipSuck",
            "v2/LipFunnelUpper",
            "v2/LipFunnelLower",
            "v2/LipFunnel",
            "v2/LipPuckerUpper",
            "v2/LipPuckerLower",
            "v2/LipPuckerRight",
            "v2/LipPuckerLeft",
            "v2/LipPucker",
            "v2/LipSuckFunnelUpper",
            "v2/LipSuckFunnelLower",
            "v2/LipSuckFunnelLowerLeft",
            "v2/LipSuckFunnelLowerRight",
            "v2/LipSuckFunnelUpperLeft",
            "v2/LipSuckFunnelUpperRight",
            "v2/MouthUpperUp",
            "v2/MouthLowerDown",
            "v2/MouthOpen",
            "v2/MouthStretch",
            "v2/MouthTightener",
            "v2/MouthPress",
            "v2/MouthDimple",
            "v2/NoseSneer",
            "v2/MouthTightenerStretch",
            "v2/MouthTightenerStretchLeft",
            "v2/MouthTightenerStretchRight",
            "v2/MouthCornerYLeft",
            "v2/MouthCornerYRight",
            "v2/MouthCornerY",
            "v2/SmileFrownRight",
            "v2/SmileFrownLeft",
            "v2/SmileFrown",
            "v2/SmileSadRight",
            "v2/SmileSadLeft",
            "v2/SmileSad",
            // Tongue computed
            "v2/TongueX",
            "v2/TongueY",
            "v2/TongueArchY",
            "v2/TongueShape",
            // Tracking active flags
            "EyeTrackingActive",
            "ExpressionTrackingActive",
            "LipTrackingActive",
            // sranipal params that may be missing
            "MouthLowerOverlay",
            "TongueLongStep1",
            "TongueLongStep2",
        ];

        for name in v2_computed_params {
            if !map.contains_key(name) {
                let addr = format!("/avatar/parameters/FT/{}", name);
                map.insert(*name, Box::leak(addr.into_boxed_str()) as &'static str);
            }
        }

        map
    });

    // Fallback for truly unknown names - no warning since it still works correctly
    cache.get(name).copied().unwrap_or_else(|| {
        let addr = format!("/avatar/parameters/FT/{}", name);
        Box::leak(addr.into_boxed_str())
    })
}
