use crate::mutator::MutationConfig;
use crate::UnifiedTrackingData;
use anyhow::Result;

pub trait Mutation: Send + Sync {
    /// Initialize the mutation with current data or config
    fn initialize(&mut self, config: &MutationConfig) -> Result<()>;

    /// Process and modify the tracking data in-place
    fn mutate(&mut self, data: &mut UnifiedTrackingData, dt: f32);

    /// Unique identifier for this mutation (e.g., "EuroFilter", "Smoothing")
    fn name(&self) -> &str;

    /// Optional: Comparison for ordering/priority
    fn priority(&self) -> i32 {
        0
    }
}
