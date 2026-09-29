//! The extensions built into this app, each behind its Cargo feature.
use vrft_gui_core::extension::Factory;

/// Every extension built in, in navigation order.
// Each push is behind its feature, so the list can be empty.
#[allow(unused_mut, clippy::vec_init_then_push)]
pub fn built_in() -> Vec<Factory> {
    let mut extensions: Vec<Factory> = Vec::new();
    #[cfg(feature = "quest-pro")]
    extensions.push(vrft_quest_pro_gui::create);
    extensions
}
