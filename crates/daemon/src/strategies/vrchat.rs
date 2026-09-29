use crate::osc::query::target::VrchatTarget;
use crate::osc::query::vrchat;
use crate::osc::vrchat::VRChatOsc;
use crate::strategies::OscContext;
use anyhow::Result;
use axum::Router;
use vrft_common::{IntegrationAdapter, UnifiedTrackingData};

pub struct VRChatOscStrategy {
    inner: VRChatOsc,
}

use std::sync::mpsc::Receiver;

impl VRChatOscStrategy {
    pub fn new(
        target: VrchatTarget,
        receive_port: u16,
        context: OscContext,
    ) -> (Self, Router, Option<Receiver<String>>) {
        let reply_ip = target.reply_ip();
        let inner = VRChatOsc::new(target, receive_port);
        // Advertise the address and port we actually listen on, not fixed
        // defaults.
        let router = vrchat::get_router(context.tracking_data, reply_ip, inner.receive_port());

        let change_rx = inner.change_rx.lock().unwrap().take();

        (Self { inner }, router, change_rx)
    }
}

impl IntegrationAdapter for VRChatOscStrategy {
    fn initialize(&mut self) -> Result<()> {
        self.inner.initialize()
    }

    fn send(&self, data: &UnifiedTrackingData) -> Result<()> {
        self.inner.send(data)
    }
}
