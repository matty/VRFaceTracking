use crate::strategies::PlatformBackend;
use anyhow::Result;
use common::{IntegrationAdapter, UnifiedTrackingData};

pub struct Dispatcher {
    backend: PlatformBackend,
}

impl Dispatcher {
    pub fn new(backend: PlatformBackend) -> Self {
        Self { backend }
    }

    pub fn initialize(&mut self) -> Result<()> {
        self.backend.initialize()
    }

    pub fn send(&self, data: &UnifiedTrackingData) -> Result<()> {
        self.backend.send(data)
    }
}