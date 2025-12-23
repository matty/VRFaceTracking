use anyhow::{Context, Result};
use common::{IntegrationAdapter, UnifiedTrackingData};
use log::info;
use std::net::UdpSocket;

pub struct GenericUdpStrategy {
    socket: Option<UdpSocket>,
    target_address: String,
}

impl GenericUdpStrategy {
    pub fn new(target_address: String) -> Self {
        Self {
            socket: None,
            target_address,
        }
    }
}

impl IntegrationAdapter for GenericUdpStrategy {
    fn initialize(&mut self) -> Result<()> {
        info!("Initializing Generic UDP Strategy...");
        // Bind to 0.0.0.0:0 to let OS pick a port
        let socket = UdpSocket::bind("0.0.0.0:0").context("Failed to bind UDP socket")?;
        socket
            .connect(&self.target_address)
            .context(format!("Failed to connect to {}", self.target_address))?;
        socket
            .set_nonblocking(true)
            .context("Failed to set non-blocking mode")?;

        self.socket = Some(socket);
        info!(
            "Generic UDP Strategy initialized. Target: {}",
            self.target_address
        );
        Ok(())
    }

    fn send(&self, data: &UnifiedTrackingData) -> Result<()> {
        if let Some(socket) = &self.socket {
            let json_data = serde_json::to_vec(data)?;
            socket.send(&json_data)?;
        }
        Ok(())
    }
}
