//! The extensions built into this daemon, each behind its Cargo feature.
use vrft_extension::DaemonExtension;

/// Every extension built in, in the order their frame hooks run.
// Each push is behind its feature, so the list can be empty.
#[allow(unused_mut, clippy::vec_init_then_push)]
pub fn built_in() -> Vec<Box<dyn DaemonExtension>> {
    let mut extensions: Vec<Box<dyn DaemonExtension>> = Vec::new();
    #[cfg(feature = "quest-pro")]
    extensions.push(Box::new(vrft_quest_pro_daemon::QuestPro));
    extensions
}
