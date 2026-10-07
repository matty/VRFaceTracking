//! Meta's own face tracking values, as the tracking module sends them, for a
//! `universal-face-v2` model. QFT+'s event layer reads them by their OpenXR
//! names: TongueOut for the tongue's visibility and extension, the brow
//! raises, cheek puffs, and the lips and jaw for its gates and speech.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, RwLock};
use std::time::Instant;

use vrft_api::{UnifiedExpressions, UnifiedTrackingData};
use vrft_tongue::universal_v2::Native;

use UnifiedExpressions as U;

/// Meta's names (`XR_FB_face_tracking2`), by the Unified Expression a Quest
/// Pro module sends each as.
const NAMES: &[(&str, UnifiedExpressions)] = &[
    ("JawDrop", U::JawOpen),
    ("LipsToward", U::MouthClosed),
    ("TongueOut", U::TongueOut),
    ("CheekPuffL", U::CheekPuffLeft),
    ("CheekPuffR", U::CheekPuffRight),
    ("CheekSuckL", U::CheekSuckLeft),
    ("CheekSuckR", U::CheekSuckRight),
    ("InnerBrowRaiserL", U::BrowInnerUpLeft),
    ("InnerBrowRaiserR", U::BrowInnerUpRight),
    ("OuterBrowRaiserL", U::BrowOuterUpLeft),
    ("OuterBrowRaiserR", U::BrowOuterUpRight),
    ("BrowLowererL", U::BrowLowererLeft),
    ("BrowLowererR", U::BrowLowererRight),
    ("JawSidewaysLeft", U::JawLeft),
    ("JawSidewaysRight", U::JawRight),
    ("JawThrust", U::JawForward),
    ("CheekRaiserL", U::CheekSquintLeft),
    ("CheekRaiserR", U::CheekSquintRight),
    ("ChinRaiserB", U::MouthRaiserLower),
    ("ChinRaiserT", U::MouthRaiserUpper),
    ("DimplerL", U::MouthDimpleLeft),
    ("DimplerR", U::MouthDimpleRight),
    ("LipCornerDepressorL", U::MouthFrownLeft),
    ("LipCornerDepressorR", U::MouthFrownRight),
    ("LipCornerPullerL", U::MouthCornerPullLeft),
    ("LipCornerPullerR", U::MouthCornerPullRight),
    ("LipFunnelerLB", U::LipFunnelLowerLeft),
    ("LipFunnelerRB", U::LipFunnelLowerRight),
    ("LipFunnelerLT", U::LipFunnelUpperLeft),
    ("LipFunnelerRT", U::LipFunnelUpperRight),
    ("LipPressorL", U::MouthPressLeft),
    ("LipPressorR", U::MouthPressRight),
    ("LipPuckerL", U::LipPuckerUpperLeft),
    ("LipPuckerR", U::LipPuckerUpperRight),
    ("LipStretcherL", U::MouthStretchLeft),
    ("LipStretcherR", U::MouthStretchRight),
    ("LipSuckLB", U::LipSuckLowerLeft),
    ("LipSuckRB", U::LipSuckLowerRight),
    ("LipSuckLT", U::LipSuckUpperLeft),
    ("LipSuckRT", U::LipSuckUpperRight),
    ("LipTightenerL", U::MouthTightenerLeft),
    ("LipTightenerR", U::MouthTightenerRight),
    ("LowerLipDepressorL", U::MouthLowerDownLeft),
    ("LowerLipDepressorR", U::MouthLowerDownRight),
    ("MouthLeft", U::MouthUpperLeft),
    ("MouthRight", U::MouthUpperRight),
    ("UpperLipRaiserL", U::MouthUpperUpLeft),
    ("UpperLipRaiserR", U::MouthUpperUpRight),
    ("NoseWrinklerL", U::NoseSneerLeft),
    ("NoseWrinklerR", U::NoseSneerRight),
];

/// Nanoseconds on one monotonic clock, from the first call: when native
/// values arrived and when a camera frame did.
pub(crate) fn nanos(at: Instant) -> i64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let epoch = *EPOCH.get_or_init(Instant::now);
    at.saturating_duration_since(epoch).as_nanos() as i64
}

/// The tracking module's latest values, kept only while a model reads them.
#[derive(Default)]
pub(crate) struct NativeFeed {
    wanted: AtomicBool,
    latest: RwLock<Option<Native>>,
}

impl NativeFeed {
    /// Starts or stops keeping values; stopping forgets the last ones.
    pub(crate) fn want(&self, wanted: bool) {
        self.wanted.store(wanted, Ordering::Relaxed);
        if !wanted {
            *self.latest.write().unwrap() = None;
        }
    }

    /// Keeps a tracking module frame's values, when wanted.
    pub(crate) fn record(&self, data: &UnifiedTrackingData, at: Instant) {
        if !self.wanted.load(Ordering::Relaxed) {
            return;
        }
        *self.latest.write().unwrap() = Some(snapshot(data, at));
    }

    pub(crate) fn latest(&self) -> Option<Native> {
        self.latest.read().unwrap().clone()
    }
}

/// Meta's values whose names start with one of `prefixes`, by name.
pub(crate) fn named(data: &UnifiedTrackingData, prefixes: &[&str]) -> Vec<(&'static str, f32)> {
    NAMES
        .iter()
        .filter(|(name, _)| prefixes.iter().any(|prefix| name.starts_with(prefix)))
        .map(|&(name, shape)| (name, data.shapes[shape as usize].weight.clamp(0.0, 1.0)))
        .collect()
}

fn snapshot(data: &UnifiedTrackingData, at: Instant) -> Native {
    let values: HashMap<String, f64> = NAMES
        .iter()
        .map(|&(name, shape)| {
            let value = data.shapes[shape as usize].weight.clamp(0.0, 1.0);
            (name.to_string(), f64::from(value))
        })
        .collect();
    Native {
        arrival: nanos(at),
        values,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_name_reads_its_own_expression() {
        let mut data = UnifiedTrackingData::default();
        for (index, &(_, shape)) in NAMES.iter().enumerate() {
            data.shapes[shape as usize].weight = index as f32 / 100.0;
        }
        let native = snapshot(&data, Instant::now());
        assert_eq!(native.values.len(), NAMES.len(), "a name is listed twice");
        for (index, &(name, _)) in NAMES.iter().enumerate() {
            let expected = f64::from(index as f32 / 100.0);
            assert_eq!(native.values[name], expected, "{name}");
        }
    }

    #[test]
    fn named_values_are_the_ones_asked_for() {
        let mut data = UnifiedTrackingData::default();
        data.shapes[U::LipPuckerUpperRight as usize].weight = 0.6;
        data.shapes[U::JawOpen as usize].weight = 0.5;
        let values = named(&data, &["LipPucker", "JawDrop"]);
        assert_eq!(
            values,
            vec![("JawDrop", 0.5), ("LipPuckerL", 0.0), ("LipPuckerR", 0.6)]
        );
    }

    #[test]
    fn values_are_kept_only_while_wanted() {
        let feed = NativeFeed::default();
        let data = UnifiedTrackingData::default();
        feed.record(&data, Instant::now());
        assert!(feed.latest().is_none());
        feed.want(true);
        feed.record(&data, Instant::now());
        assert!(feed.latest().is_some());
        feed.want(false);
        assert!(feed.latest().is_none());
    }

    #[test]
    fn the_clock_runs_forward() {
        let first = nanos(Instant::now());
        let later = nanos(Instant::now() + std::time::Duration::from_millis(5));
        assert!(later - first >= 5_000_000);
    }
}
