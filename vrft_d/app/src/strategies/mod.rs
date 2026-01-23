pub mod generic_udp;
pub mod resonite;
pub mod vrchat;

use anyhow::Result;
use axum::Router;
use common::{IntegrationAdapter, MutationConfig, OutputMode, UnifiedTrackingData};
use generic_udp::GenericUdpStrategy;
use resonite::ResoniteOscStrategy;
use std::sync::{Arc, RwLock};
use vrchat::VRChatOscStrategy;

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
                config.osc.send_port + 1,
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
