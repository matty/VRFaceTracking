//! Quest Pro tongue and cheek puff models: loading, live inference and
//! personal training, in Rust with Burn. No Python runtime is needed.

pub mod backend;
pub mod checkpoint;
pub mod dataset;
pub mod infer;
pub mod model;
pub mod preprocess;
pub mod recordings;
pub mod train;

pub use backend::Accelerator;
pub use checkpoint::{Checkpoint, Role};
pub use infer::{ModelInfo, TongueModel};

/// Model outputs, in order: ten tongue heads, then each cheek's puff.
pub const TARGETS: [&str; 12] = [
    "visibility",
    "extension",
    "horizontal",
    "vertical",
    "curl_up",
    "bend_down",
    "roll",
    "flat",
    "squish",
    "twist",
    "cheek_puff_left",
    "cheek_puff_right",
];

/// The tongue heads, which the built-in pair and recordings made before
/// cheek puffs were added have alone.
pub const TONGUE_TARGETS: usize = 10;

/// The cheek puff heads. Unlike the tongue heads, they are labelled whether
/// or not the tongue is out.
pub const CHEEK_COLUMNS: [usize; 2] = [10, 11];
