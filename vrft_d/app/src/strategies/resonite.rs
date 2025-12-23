use crate::osc::resonite::ResoniteOsc;
use anyhow::Result;
use common::{IntegrationAdapter, UnifiedTrackingData};

pub struct ResoniteOscStrategy {
    inner: ResoniteOsc,
}

impl ResoniteOscStrategy {
    pub fn new(target_addr: &str) -> Self {
        let inner = ResoniteOsc::new(target_addr);
        Self { inner }
    }
}

impl IntegrationAdapter for ResoniteOscStrategy {
    fn initialize(&mut self) -> Result<()> {
        self.inner.initialize()
    }

    fn send(&self, data: &UnifiedTrackingData) -> Result<()> {
        self.inner.send(data)
    }
}