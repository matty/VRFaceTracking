pub mod generic_udp;

use crate::osc::query::target::VrchatTarget;
use crate::osc::query::vrchat;
use crate::osc::resonite::ResoniteOsc;
use crate::osc::vrchat::VRChatOsc;
use anyhow::Result;
use axum::Router;
use generic_udp::GenericUdpStrategy;
use std::sync::{Arc, RwLock};
use vrft_common::{IntegrationAdapter, MutationConfig, OscConfig, OutputMode, UnifiedTrackingData};

pub struct OscContext {
    pub tracking_data: Arc<RwLock<UnifiedTrackingData>>,
    /// Where VRChat tracking goes, updated as OSCQuery finds VRChat.
    pub vrchat: VrchatTarget,
}

impl IntegrationAdapter for VRChatOsc {
    fn initialize(&mut self) -> Result<()> {
        VRChatOsc::initialize(self)
    }

    fn send(&self, data: &UnifiedTrackingData) -> Result<()> {
        VRChatOsc::send(self, data)
    }

    fn stop(&self) {
        self.send_neutral_face();
    }
}

impl IntegrationAdapter for ResoniteOsc {
    fn initialize(&mut self) -> Result<()> {
        ResoniteOsc::initialize(self)
    }

    fn send(&self, data: &UnifiedTrackingData) -> Result<()> {
        ResoniteOsc::send(self, data)
    }
}

/// `host:port` for `osc`'s send address, with an IPv6 address in brackets.
fn send_target(osc: &OscConfig) -> String {
    if osc.send_address.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{}]:{}", osc.send_address, osc.send_port)
    } else {
        format!("{}:{}", osc.send_address, osc.send_port)
    }
}

/// What VRChat's output needs of the OSCQuery host.
pub struct OscQueryAdvert {
    pub router: Router,
    /// The UDP port OSC arrives on.
    pub osc_port: u16,
}

/// The output `config.osc.output_mode` asks for, and for VRChat, how to
/// advertise it over OSCQuery.
pub fn create_strategy(
    config: &MutationConfig,
    context: OscContext,
) -> (Box<dyn IntegrationAdapter>, Option<OscQueryAdvert>) {
    match config.osc.output_mode {
        OutputMode::Generic => (
            Box::new(GenericUdpStrategy::new(send_target(&config.osc))),
            None,
        ),
        OutputMode::VRChat => {
            let reply_ip = context.vrchat.reply_ip();
            let osc = VRChatOsc::new(context.vrchat);
            let osc_port = osc.receive_port();
            let router = vrchat::get_router(context.tracking_data, reply_ip, osc_port);
            (Box::new(osc), Some(OscQueryAdvert { router, osc_port }))
        }
        OutputMode::Resonite => (Box::new(ResoniteOsc::new(&send_target(&config.osc))), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv6_send_addresses_are_bracketed() {
        let osc = |address: &str| OscConfig {
            send_address: address.into(),
            send_port: 9000,
            ..OscConfig::default()
        };
        assert_eq!(send_target(&osc("127.0.0.1")), "127.0.0.1:9000");
        assert_eq!(send_target(&osc("::1")), "[::1]:9000");
        assert_eq!(send_target(&osc("localhost")), "localhost:9000");
    }
}
