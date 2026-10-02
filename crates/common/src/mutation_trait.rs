use crate::UnifiedTrackingData;

/// One step of the mutation pipeline, built from the config by its `new`.
pub trait Mutation: Send + Sync {
    /// Process and modify the tracking data in-place
    fn mutate(&mut self, data: &mut UnifiedTrackingData, dt: f32);

    /// Unique identifier for this mutation (e.g., "EuroFilter", "Smoothing")
    fn name(&self) -> &str;
}
