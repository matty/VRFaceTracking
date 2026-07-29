pub mod generic_udp;
pub mod resonite;
pub mod vrchat;

use anyhow::Result;
use axum::Router;
use generic_udp::GenericUdpStrategy;
use resonite::ResoniteOscStrategy;
use std::sync::{Arc, RwLock};
use vrchat::VRChatOscStrategy;
use vrft_common::{IntegrationAdapter, MutationConfig, OutputMode, UnifiedTrackingData};

pub struct OscContext {
    pub tracking_data: Arc<RwLock<UnifiedTrackingData>>,
}

pub enum PlatformBackend {
    VRChat(Box<VRChatOscStrategy>),
    Resonite(ResoniteOscStrategy),
    Generic(GenericUdpStrategy),
}

impl IntegrationAdapter for PlatformBackend {
    fn initialize(&mut self) -> Result<()> {
        match self {
            Self::VRChat(s) => s.initialize(),
            Self::Resonite(s) => s.initialize(),
            Self::Generic(s) => s.initialize(),
        }
    }

    fn send(&self, data: &UnifiedTrackingData) -> Result<()> {
        match self {
            Self::VRChat(s) => s.send(data),
            Self::Resonite(s) => s.send(data),
            Self::Generic(s) => s.send(data),
        }
    }
}

use std::sync::mpsc::Receiver;

/// Port used to listen for OSC messages when one cannot be derived from the
/// send port.
const FALLBACK_RECEIVE_PORT: u16 = 9001;

/// The receive port is conventionally one above the send port. Guard the
/// addition so a send port of 65535 cannot overflow.
fn receive_port_for(send_port: u16) -> u16 {
    send_port.checked_add(1).unwrap_or_else(|| {
        log::warn!(
            "osc.send_port is {}, cannot use send_port + 1 for the OSC listener; falling back to {}",
            send_port,
            FALLBACK_RECEIVE_PORT
        );
        FALLBACK_RECEIVE_PORT
    })
}

pub fn create_strategy(
    config: &MutationConfig,
    context: OscContext,
) -> (PlatformBackend, Option<Router>, Option<Receiver<String>>) {
    match config.osc.output_mode {
        OutputMode::Generic => (
            PlatformBackend::Generic(GenericUdpStrategy::new(format!(
                "{}:{}",
                config.osc.send_address, config.osc.send_port
            ))),
            None,
            None,
        ),
        OutputMode::VRChat => {
            let (strategy, router, change_rx) = VRChatOscStrategy::new(
                &format!("{}:{}", config.osc.send_address, config.osc.send_port),
                receive_port_for(config.osc.send_port),
                context,
            );
            (
                PlatformBackend::VRChat(Box::new(strategy)),
                Some(router),
                change_rx,
            )
        }
        OutputMode::Resonite => {
            let strategy = ResoniteOscStrategy::new(&format!(
                "{}:{}",
                config.osc.send_address, config.osc.send_port
            ));
            (PlatformBackend::Resonite(strategy), None, None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receive_port_is_one_above_send_port() {
        assert_eq!(receive_port_for(9000), 9001);
        assert_eq!(receive_port_for(9100), 9101);
    }

    #[test]
    fn receive_port_does_not_overflow_at_the_maximum_send_port() {
        assert_eq!(receive_port_for(u16::MAX), FALLBACK_RECEIVE_PORT);
    }
}
