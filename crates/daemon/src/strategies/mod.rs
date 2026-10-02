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
}

impl IntegrationAdapter for ResoniteOsc {
    fn initialize(&mut self) -> Result<()> {
        ResoniteOsc::initialize(self)
    }

    fn send(&self, data: &UnifiedTrackingData) -> Result<()> {
        ResoniteOsc::send(self, data)
    }
}

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

/// `host:port` for `osc`'s send address, with an IPv6 address in brackets.
fn send_target(osc: &OscConfig) -> String {
    if osc.send_address.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{}]:{}", osc.send_address, osc.send_port)
    } else {
        format!("{}:{}", osc.send_address, osc.send_port)
    }
}

/// The output `config.osc.output_mode` asks for, and for VRChat, the
/// OSCQuery routes that advertise it.
pub fn create_strategy(
    config: &MutationConfig,
    context: OscContext,
) -> (Box<dyn IntegrationAdapter>, Option<Router>) {
    match config.osc.output_mode {
        OutputMode::Generic => (
            Box::new(GenericUdpStrategy::new(send_target(&config.osc))),
            None,
        ),
        OutputMode::VRChat => {
            let reply_ip = context.vrchat.reply_ip();
            let osc = VRChatOsc::new(context.vrchat, receive_port_for(config.osc.send_port));
            // Advertise the address and port we actually listen on, not fixed
            // defaults.
            let router = vrchat::get_router(context.tracking_data, reply_ip, osc.receive_port());
            (Box::new(osc), Some(router))
        }
        OutputMode::Resonite => (Box::new(ResoniteOsc::new(&send_target(&config.osc))), None),
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
