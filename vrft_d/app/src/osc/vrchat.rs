use crate::osc::parameters::registry::ParameterRegistry;
use crate::osc::parameters::ParamType;
use crate::osc::query::service::{OscParamType, OscParameterInfo, OscQueryService};
use anyhow::Result;
use common::UnifiedTrackingData;
use log::{error, info};
use rosc::{decoder, encoder, OscBundle, OscPacket, OscType};
use std::collections::{HashMap, HashSet};
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub struct VRChatOsc {
    socket: Mutex<Option<UdpSocket>>,
    target_addr: String,
    receive_port: u16,
    osc_query_service: Mutex<Option<OscQueryService>>,
    query_rx: Mutex<Receiver<Option<Vec<OscParameterInfo>>>>,
    change_tx_calibration: Sender<String>,
    change_tx_query: Sender<String>,
    pub change_rx: Mutex<Option<Receiver<String>>>,
    pub param_registry: Mutex<ParameterRegistry>,
    shutdown_flag: Arc<AtomicBool>,
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
            osc_query_service: Mutex::new(Some(osc_query_service)),
            query_rx: Mutex::new(query_rx),
            change_tx_calibration,
            change_tx_query,
            change_rx: Mutex::new(Some(change_rx_calibration)),
            param_registry: Mutex::new(ParameterRegistry::new()),
            shutdown_flag: Arc::new(AtomicBool::new(false)),
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
        // Set socket timeout for graceful shutdown (500ms)
        recv_socket.set_read_timeout(Some(Duration::from_millis(500)))?;

        let tx_calib = self.change_tx_calibration.clone();
        let tx_query = self.change_tx_query.clone();
        let port = self.receive_port;
        let shutdown = self.shutdown_flag.clone();

        thread::spawn(move || {
            info!("Listening for OSC messages on port {}", port);
            let mut buf = [0u8; 2048];
            while !shutdown.load(Ordering::Relaxed) {
                match recv_socket.recv_from(&mut buf) {
                    Ok((size, _addr)) => {
                        if let Ok((_, packet)) = decoder::decode_udp(&buf[..size]) {
                            handle_packet(packet, &tx_calib, &tx_query);
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
        });

        *self.socket.lock().unwrap() = Some(socket);
        Ok(())
    }

    pub fn send(&self, data: &UnifiedTrackingData) -> Result<()> {
        // Check for parameter updates (quick lock)
        if let Ok(rx) = self.query_rx.lock() {
            while let Ok(update) = rx.try_recv() {
                // Convert Vec<OscParameterInfo> to the structures we need
                if let Some(param_infos) = &update {
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

                    let bool_count = param_types
                        .values()
                        .filter(|t| **t == ParamType::Bool)
                        .count();
                    let float_count = param_types
                        .values()
                        .filter(|t| **t == ParamType::Float)
                        .count();
                    let int_count = param_types
                        .values()
                        .filter(|t| **t == ParamType::Int)
                        .count();
                    info!(
                        "OSC Query Types: {} bool, {} float, {} int",
                        bool_count, float_count, int_count
                    );

                    // Debug: Log sample of avatar params to verify format
                    let sample_params: Vec<_> = avatar_params.iter().take(10).collect();
                    log::debug!("Sample avatar params: {:?}", sample_params);

                    let ft_sample: Vec<_> = avatar_params
                        .iter()
                        .filter(|p| p.contains("FT") || p.contains("v2"))
                        .take(10)
                        .collect();
                    log::info!("FT/v2 sample params: {:?}", ft_sample);

                    // Reset parameter registry with real types from OSC Query
                    if let Ok(mut registry) = self.param_registry.lock() {
                        log::info!("Calling registry.reset()...");
                        registry.reset(&avatar_params, &param_types);
                        log::info!("registry.reset() completed");
                    } else {
                        log::error!("Failed to acquire param_registry lock!");
                    }
                }
            }
        }

        let messages = {
            let mut registry = self.param_registry.lock().unwrap();
            registry.process(data)
        };

        if messages.is_empty() {
            return Ok(());
        }

        // Encode the bundle
        let bundle = OscBundle {
            timetag: rosc::OscTime::from((0, 0)),
            content: messages.into_iter().map(OscPacket::Message).collect(),
        };
        let packet = OscPacket::Bundle(bundle);
        let msg_buf = encoder::encode(&packet)?;

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

    /// Signal the OSC listener thread to shut down gracefully
    pub fn shutdown(&self) {
        info!("Shutting down OSC listener...");
        self.shutdown_flag.store(true, Ordering::Relaxed);
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
