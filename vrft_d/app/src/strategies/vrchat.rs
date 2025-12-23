use crate::osc::query::vrchat;
use crate::osc::vrchat::VRChatOsc;
use crate::strategies::OscContext;
use anyhow::Result;
use axum::Router;
use common::{IntegrationAdapter, UnifiedTrackingData};

pub struct VRChatOscStrategy {
    inner: VRChatOsc,
}

use std::sync::mpsc::Receiver;

impl VRChatOscStrategy {
    pub fn new(
        target_addr: &str,
        receive_port: u16,
        context: OscContext,
    ) -> (Self, Router, Option<Receiver<String>>) {
        let inner = VRChatOsc::new(target_addr, receive_port);
        let router = vrchat::get_router(context.tracking_data, 9001);

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
