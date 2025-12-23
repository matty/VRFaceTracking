// Thanks to "VRCFTPicoModule" for the initial implementation
// https://github.com/lonelyicer/VRCFTPicoModule

use anyhow::Result;
use api::{ModuleLogger, TrackingModule, UnifiedTrackingData};
use std::net::UdpSocket;
use std::time::Duration;

use crate::data::{DataPackBody, DataPackHeader};
use crate::mapping::update_face_data;

const PORT_STANDARD: u16 = 29765;
const PORT_LEGACY: u16 = 29763;

pub struct PicoModule {
    socket: Option<UdpSocket>,
    logger: Option<ModuleLogger>,
    buf: [u8; 2048],
    is_legacy: bool,
}

impl PicoModule {
    pub fn new() -> Self {
        Self {
            socket: None,
            logger: None,
            buf: [0; 2048],
            is_legacy: false,
        }
    }

    fn connect(&mut self) -> Result<()> {
        // Try standard port first
        if let Ok(socket) = UdpSocket::bind(format!("0.0.0.0:{}", PORT_STANDARD)) {
            socket.set_read_timeout(Some(Duration::from_millis(10)))?;
            self.socket = Some(socket);
            self.is_legacy = false;
            if let Some(logger) = &self.logger {
                logger.info(&format!("Listening on standard port {}", PORT_STANDARD));
            }
            return Ok(());
        }

        // Try legacy port
        if let Ok(socket) = UdpSocket::bind(format!("0.0.0.0:{}", PORT_LEGACY)) {
            socket.set_read_timeout(Some(Duration::from_millis(10)))?;
            self.socket = Some(socket);
            self.is_legacy = true;
            if let Some(logger) = &self.logger {
                logger.info(&format!("Listening on legacy port {}", PORT_LEGACY));
            }
            return Ok(());
        }

        Err(anyhow::anyhow!("Failed to bind to any Pico port"))
    }
}

impl TrackingModule for PicoModule {
    fn initialize(&mut self, logger: ModuleLogger) -> Result<()> {
        logger.info("Initializing Pico Module");

        // Run auto-configuration
        crate::config_setup::setup_pico_connect(&logger);

        self.logger = Some(logger);

        match self.connect() {
            Ok(_) => Ok(()),
            Err(e) => {
                if let Some(logger) = &self.logger {
                    logger.error(&format!("Failed to initialize Pico module: {}", e));
                }
                Err(e)
            }
        }
    }

    fn update(&mut self, data: &mut UnifiedTrackingData) -> Result<()> {
        if let Some(socket) = &self.socket {
            match socket.recv_from(&mut self.buf) {
                Ok((amt, _src)) => {
                    let packet_data = &self.buf[..amt];

                    let weights = if self.is_legacy {
                        // Legacy packet: Just the body
                        if amt >= std::mem::size_of::<DataPackBody>() {
                            let body: DataPackBody =
                                unsafe { std::ptr::read(packet_data.as_ptr() as *const _) };
                            Some(body.blend_shapes)
                        } else {
                            None
                        }
                    } else {
                        // Standard packet: Header + Body
                        let header_size = std::mem::size_of::<DataPackHeader>();
                        if amt >= header_size + std::mem::size_of::<DataPackBody>() {
                            let header: DataPackHeader =
                                unsafe { std::ptr::read(packet_data.as_ptr() as *const _) };
                            // Check tracking type (2 = Face)
                            if header.tracking_type == 2 {
                                let body_ptr = unsafe { packet_data.as_ptr().add(header_size) };
                                let body: DataPackBody =
                                    unsafe { std::ptr::read(body_ptr as *const _) };
                                Some(body.blend_shapes)
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    };

                    if let Some(w) = weights {
                        update_face_data(data, &w);
                        return Ok(());
                    }
                }
                Err(e) => {
                    if e.kind() != std::io::ErrorKind::WouldBlock
                        && e.kind() != std::io::ErrorKind::TimedOut
                    {
                        if let Some(logger) = &self.logger {
                            logger.warn(&format!("UDP Receive Error: {}", e));
                        }
                    }
                }
            }
        }

        // No new data, but not a fatal error
        Ok(())
    }

    fn unload(&mut self) {
        if let Some(logger) = &self.logger {
            logger.info("Unloading Pico Module");
        }
        self.socket = None;
    }
}
