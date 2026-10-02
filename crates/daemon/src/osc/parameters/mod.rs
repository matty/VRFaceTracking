pub mod base_param;
pub mod binary_param;
pub mod eparam;
pub mod legacy_eye;
pub mod legacy_lip;
pub mod native_param;
pub mod registry;

use rosc::OscMessage;
use std::collections::{HashMap, HashSet};
use vrft_common::UnifiedTrackingData;

/// The address prefix VRChat gives every avatar parameter.
pub(crate) const DEFAULT_PREFIX: &str = "/avatar/parameters/";

/// Whether the last path segment of `prefix` is a version marker (`v1`, `v2`,
/// `v10`...), the text in front of `/{name}` in an address.
///
/// A parameter found behind one belongs to a different generation than the
/// one being matched, so `v1/v2/EyeLeftX` is not `v2/EyeLeftX` and
/// `v2/JawOpen` is not the legacy `JawOpen`. Only a whole segment counts:
/// `Dev2/JawOpen` and `Av3/JawOpen` are ordinary prefixes.
pub(crate) fn ends_with_version_segment(prefix: &str) -> bool {
    let segment = prefix.rsplit('/').next().unwrap_or(prefix);
    segment
        .strip_prefix('v')
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

/// Parameter type information from avatar
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamType {
    Float,
    Bool,
    Int,
}

/// Trait for all parameter types
pub trait Parameter: Send + Sync {
    /// Reset parameter state based on avatar's available parameters.
    /// Returns the count of the avatar's own parameters this now drives, which
    /// is how the caller tells whether the avatar has face tracking at all.
    fn reset(
        &mut self,
        avatar_params: &HashSet<String>,
        param_types: &HashMap<String, ParamType>,
    ) -> usize;

    /// Process tracking data and return OSC messages to send
    fn process(&mut self, data: &UnifiedTrackingData) -> Vec<OscMessage>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_version_segments_are_recognised() {
        for prefix in ["v1", "v2", "v10", "FT/v2", "v10/v2", "OSCm/Float/v3"] {
            assert!(ends_with_version_segment(prefix), "{prefix}");
        }
    }

    /// The old check only looked at the last two characters, so a prefix that
    /// merely ended in `v` and a digit was rejected and `v10` was not.
    #[test]
    fn only_a_whole_segment_is_a_version() {
        for prefix in [
            "", "FT", "Dev2", "Av3", "v", "vv", "v2a", "V2", "v2/FT", "2",
        ] {
            assert!(!ends_with_version_segment(prefix), "{prefix}");
        }
    }
}
