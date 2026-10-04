//! The universal face model, `universal-face-v1`: one network for everyone
//! over all five Quest Pro cameras, conditioned on the wearer's one-minute
//! face setup. It sits beside the stereo tongue pair, which keeps running
//! whenever the headset sends only the mouth cameras.
//!
//! The design follows QFT+'s "universal face" model (MIT license), rebuilt
//! in Burn and trained from VRFT's own v8 encoder rather than QFT+'s weights:
//!
//! - Each camera view, shrunk to [`IMAGE_SIZE`] px, goes through a shared
//!   front: the first three stages of the v8 encoder (`encoder.network.0-11`,
//!   96 x 16 x 16 out of a 128 px view).
//! - Three tails, each its own copy of the encoder's last stage (96 -> 160):
//!   - mouth (cameras 2 and 3): pooled to 4 x 4, a shared 2560 -> 256 linear
//!     layer plus a learned offset per camera, giving a 512-wide embedding;
//!   - tongue (cameras 2 and 3): global mean, 320 -> visibility, extension,
//!     horizontal and vertical;
//!   - brow (cameras 0, 1 and 4): global mean, a 480-wide embedding.
//! - Two heads conditioned on the wearer's enrolled poses ([`ANCHOR_SLOTS`]),
//!   each with a learned vector standing in for any pose not enrolled:
//!   - mouth: `[q, six anchors, q - neutral, six present flags]` (4102) ->
//!     512 -> 256 -> [`MOUTH_OUTPUTS`];
//!   - brow: `[w, w - neutral w, present]` (961) -> 128 -> [`BROW_OUTPUTS`].
//!
//! Inputs are in 0..1, as the v8 encoder was trained on, not QFT+'s
//! `(x - 0.25) / 0.25`.

pub mod checkpoint;
pub mod data;
pub mod infer;
pub mod net;
pub mod train;

pub use checkpoint::{FaceCheckpoint, FaceMetadata};
pub use infer::{Enrollment, FaceModel, FacePrediction};

/// The architecture's name in checkpoints and training requests.
pub const ARCHITECTURE: &str = "universal-face-v1";
/// Where a model folder keeps it.
pub const FILE_NAME: &str = "universal-face-v1.safetensors";
/// Each camera view's size at the model's input.
pub const IMAGE_SIZE: usize = 128;
pub const CAMERAS: usize = 5;
/// Width of the mouth embedding (both cameras).
pub const MOUTH_EMBEDDING: usize = 512;
/// Width of the brow embedding (three cameras).
pub const BROW_EMBEDDING: usize = 480;
/// The wearer's enrolled poses, in the order the mouth head reads them.
pub const ANCHOR_SLOTS: [&str; 6] = [
    "neutral",
    "jaw_open",
    "pucker",
    "puff",
    "tongue_out",
    "suck",
];
/// Tongue outputs, as the stereo pair's first four heads.
pub const TONGUE_OUTPUTS: [&str; 4] = ["visibility", "extension", "horizontal", "vertical"];
pub const MOUTH_OUTPUTS: [&str; 5] = [
    "cheek_puff_left",
    "cheek_puff_right",
    "cheek_suck_left",
    "cheek_suck_right",
    "jaw_open",
];
pub const BROW_OUTPUTS: [&str; 8] = [
    "brow_inner_up_left",
    "brow_inner_up_right",
    "brow_outer_up_left",
    "brow_outer_up_right",
    "brow_lowerer_left",
    "brow_lowerer_right",
    "brow_pinch_left",
    "brow_pinch_right",
];
/// Every output, in the order predictions list them: the tongue, the mouth
/// head, then the brow head.
pub const FACE_TARGETS: [&str; 17] = [
    "visibility",
    "extension",
    "horizontal",
    "vertical",
    "cheek_puff_left",
    "cheek_puff_right",
    "cheek_suck_left",
    "cheek_suck_right",
    "jaw_open",
    "brow_inner_up_left",
    "brow_inner_up_right",
    "brow_outer_up_left",
    "brow_outer_up_right",
    "brow_lowerer_left",
    "brow_lowerer_right",
    "brow_pinch_left",
    "brow_pinch_right",
];
/// Where the mouth and brow heads' outputs start in [`FACE_TARGETS`].
pub const MOUTH_START: usize = TONGUE_OUTPUTS.len();
pub const BROW_START: usize = MOUTH_START + MOUTH_OUTPUTS.len();
/// The signed outputs, horizontal and vertical; the rest are 0..1.
pub const SIGNED: [usize; 2] = [2, 3];

const _: () = assert!(BROW_START + BROW_OUTPUTS.len() == FACE_TARGETS.len());

/// The anchor slot a recorded pose shows, for recordings made before the
/// face setup, whose poses carry no slot of their own.
pub fn slot_for_pose(pose: &str) -> Option<usize> {
    let slot = match pose {
        "Neutral" | "Relax and look ahead" => "neutral",
        "Jaw open, no tongue" => "jaw_open",
        "Pucker, no tongue" => "pucker",
        "Both cheeks puffed" | "Cheeks puffed" => "puff",
        "Tongue straight out" => "tongue_out",
        "Cheeks sucked in" => "suck",
        _ => return None,
    };
    ANCHOR_SLOTS.iter().position(|name| *name == slot)
}

pub fn slot_index(name: &str) -> Option<usize> {
    ANCHOR_SLOTS.iter().position(|slot| *slot == name)
}

pub fn target_index(name: &str) -> Option<usize> {
    FACE_TARGETS.iter().position(|target| *target == name)
}
