//! QFT+'s face event layer (`face_events.py`, MIT), ported line for line so
//! VRFT gives the same outputs from QFT+'s model as QFT+ does. Per frame, for
//! each configured channel, in this order:
//!
//! 1. neutral-anchored offset and bounded gain: `clip((raw - neutral) *
//!    gain, 0, 1)`, `gain = 1 / (reach - neutral)` in 0.7..1.5;
//! 2. soft gates from Meta's native outputs (puff and suck need sealed lips
//!    and no tongue); stale or missing native passes;
//! 3. mutually exclusive shapes (puff against suck): the weaker can't fire;
//! 4. a speech-aware threshold raise and longer onset (speech is 2-7 Hz jaw
//!    motion in native JawDrop over 1 s);
//! 5. an event state machine: on above `on` held `hold_on` seconds, off
//!    below `off` held `hold_off` seconds;
//! 6. One Euro smoothing of the intensity while on; 0 while off.
//!
//! Neutral refreshes slowly on quiet frames and resets after a donning gap.
//! Continuous channels (brows) skip 3-5.

use std::collections::{HashMap, VecDeque};

const DONNING_GAP_NS: i64 = 2_000_000_000;
const NEUTRAL_SECONDS: f64 = 20.0;
pub const LOWER_FACE: &[&str] = &[
    "Jaw", "Lip", "Mouth", "Cheek", "Chin", "Dimpler", "Tongue", "LowerLip", "UpperLip",
];
pub const NATIVE_BROWS: &[&str] = &["InnerBrow", "OuterBrow", "BrowLowerer"];
const LIPS_APART: (f64, f64) = (0.2, 0.4);
const TONGUE_OUT: (f64, f64) = (0.3, 0.7);

#[derive(Clone, Copy, Debug)]
pub struct Event {
    pub on: f64,
    pub off: f64,
    pub hold_on: f64,
    pub hold_off: f64,
    pub speech_raise: f64,
    pub speech_hold: f64,
    /// Needs sealed lips and no tongue (Meta's native values).
    pub sealed: bool,
    /// The channels, by name prefix, that beat this one when stronger.
    pub exclusive: Option<&'static str>,
    pub continuous: bool,
    /// Native name prefixes that must be at rest to refresh the neutral.
    pub quiet: &'static [&'static str],
}

const fn event(on: f64, off: f64, hold_on: f64) -> Event {
    Event {
        on,
        off,
        hold_on,
        hold_off: 0.0,
        speech_raise: 0.0,
        speech_hold: 0.0,
        sealed: false,
        exclusive: None,
        continuous: false,
        quiet: LOWER_FACE,
    }
}

pub const PUFF: Event = Event {
    hold_off: 0.15,
    speech_raise: 0.2,
    sealed: true,
    exclusive: Some("CheekSuck"),
    ..event(0.5, 0.35, 0.12)
};
pub const SUCK: Event = Event {
    exclusive: Some("CheekPuff"),
    ..PUFF
};
pub const TONGUE: Event = Event {
    speech_hold: 0.3,
    ..event(0.5, 0.42, 0.15)
};
pub const BROW: Event = Event {
    continuous: true,
    quiet: NATIVE_BROWS,
    ..event(0.5, 0.5, 0.0)
};

fn ramp(value: f64, (low, high): (f64, f64)) -> f64 {
    ((value - low) / (high - low)).clamp(0.0, 1.0)
}

/// One Euro filter of one value (`eye_signal_filter.OneEuroVectorFilter`).
#[derive(Clone, Debug)]
pub struct OneEuro {
    min_cutoff: f64,
    beta: f64,
    derivative_cutoff: f64,
    value: Option<f64>,
    derivative: f64,
    time: Option<f64>,
}

fn alpha(cutoff: f64, elapsed: f64) -> f64 {
    let rate = 2.0 * std::f64::consts::PI * cutoff * elapsed;
    rate / (rate + 1.0)
}

impl OneEuro {
    /// The face event layer's: 1.5 Hz minimum cutoff, beta 0.5.
    pub fn face() -> Self {
        Self {
            min_cutoff: 1.5,
            beta: 0.5,
            derivative_cutoff: 1.0,
            value: None,
            derivative: 0.0,
            time: None,
        }
    }

    pub fn update(&mut self, current: f64, time: f64) -> f64 {
        let (Some(value), Some(last)) = (self.value, self.time) else {
            self.value = Some(current);
            self.time = Some(time);
            return current;
        };
        let elapsed = time - last;
        self.time = Some(time);
        if !elapsed.is_finite() || elapsed <= 1e-6 {
            return value;
        }
        let elapsed = elapsed.min(0.25);
        let raw_derivative = (current - value) / elapsed;
        self.derivative +=
            alpha(self.derivative_cutoff, elapsed) * (raw_derivative - self.derivative);
        let cutoff = self.min_cutoff + self.beta * self.derivative.abs();
        let value = value + alpha(cutoff, elapsed) * (current - value);
        self.value = Some(value);
        value
    }
}

/// Speaking moves the jaw at syllable rate: 2-7 Hz, 4-14 mean crossings a
/// second, with some amplitude.
#[derive(Default)]
struct Speech {
    samples: VecDeque<(i64, f64)>,
}

const SPEECH_WINDOW_NS: i64 = 1_000_000_000;

impl Speech {
    fn add(&mut self, time: i64, jaw: f64) {
        if self.samples.back().is_some_and(|&(last, _)| time <= last) {
            return;
        }
        self.samples.push_back((time, jaw));
        while self
            .samples
            .front()
            .is_some_and(|&(t, _)| t < time - SPEECH_WINDOW_NS)
        {
            self.samples.pop_front();
        }
    }

    fn active(&self, now: i64) -> bool {
        let s: Vec<(i64, f64)> = self
            .samples
            .iter()
            .copied()
            .filter(|&(t, _)| t >= now - SPEECH_WINDOW_NS)
            .collect();
        if s.len() < 10 {
            return false;
        }
        let mean = s.iter().map(|&(_, v)| v).sum::<f64>() / s.len() as f64;
        let v: Vec<f64> = s.iter().map(|&(_, v)| v - mean).collect();
        let seconds = (s[s.len() - 1].0 - s[0].0) as f64 / 1e9;
        let crossings = v
            .windows(2)
            .filter(|pair| pair[0].is_sign_negative() != pair[1].is_sign_negative())
            .count();
        let std = (v.iter().map(|x| x * x).sum::<f64>() / v.len() as f64).sqrt();
        let rate = crossings as f64 / seconds;
        std > 0.04 && seconds > 0.5 && (4.0..=14.0).contains(&rate)
    }
}

pub struct FaceEvents {
    /// Channel name and its event, in the order QFT+ configures them.
    config: Vec<(String, Event)>,
    anchor_neutral: HashMap<String, f64>,
    reach: HashMap<String, f64>,
    bias: f64,
    speech: Speech,
    pub speaking: bool,
    last_native: Option<i64>,
    last: Option<i64>,
    pub neutral: HashMap<String, f64>,
    pub on: HashMap<String, bool>,
    since: HashMap<String, Option<i64>>,
    filters: HashMap<String, OneEuro>,
}

impl FaceEvents {
    pub fn new(
        config: Vec<(String, Event)>,
        neutral: HashMap<String, f64>,
        reach: HashMap<String, f64>,
        bias: f64,
    ) -> Self {
        let mut events = Self {
            config,
            anchor_neutral: neutral,
            reach,
            bias: bias.clamp(-0.1, 0.1),
            speech: Speech::default(),
            speaking: false,
            last_native: None,
            last: None,
            neutral: HashMap::new(),
            on: HashMap::new(),
            since: HashMap::new(),
            filters: HashMap::new(),
        };
        events.reset();
        events
    }

    /// Donning or restart: neutral back to the face setup's, every event off.
    pub fn reset(&mut self) {
        for (name, _) in &self.config {
            self.neutral.insert(
                name.clone(),
                self.anchor_neutral.get(name).copied().unwrap_or(0.0),
            );
            self.on.insert(name.clone(), false);
            self.since.insert(name.clone(), None);
            self.filters.insert(name.clone(), OneEuro::face());
        }
    }

    /// `values` (name, raw) in order; `arrival` is when the latest native
    /// sample arrived and `native` its values while fresh. Returns every
    /// value, the configured ones processed.
    pub fn step(
        &mut self,
        values: &[(String, f64)],
        arrival: Option<i64>,
        native: Option<&HashMap<String, f64>>,
        now: i64,
    ) -> Vec<(String, f64)> {
        let dt = self
            .last
            .map_or(0.0, |last| ((now - last) as f64 / 1e9).max(0.0));
        self.last = Some(now);
        if let (Some(native), Some(arrival)) = (native, arrival) {
            if self
                .last_native
                .is_some_and(|last| arrival - last > DONNING_GAP_NS)
            {
                self.reset();
            }
            self.last_native = Some(arrival);
            if let Some(&jaw) = native.get("JawDrop") {
                self.speech.add(arrival, jaw);
            }
        }
        self.speaking = native.is_some() && self.speech.active(now);
        let at_rest = |prefixes: &[&str]| {
            let Some(native) = native else {
                return false;
            };
            let v: Vec<f64> = native
                .iter()
                .filter(|(k, _)| prefixes.iter().any(|p| k.starts_with(p)))
                .map(|(_, &v)| v)
                .collect();
            !v.is_empty() && v.iter().filter(|&&x| x < 0.15).count() as f64 / v.len() as f64 >= 0.8
        };

        let config: HashMap<&str, Event> =
            self.config.iter().map(|(n, e)| (n.as_str(), *e)).collect();
        let mut post: Vec<(String, f64)> = vec![];
        for (name, raw) in values {
            let Some(&e) = config.get(name.as_str()) else {
                continue;
            };
            let n = self.neutral[name];
            let reach = self.reach.get(name).copied().unwrap_or(1.0);
            let gain = (1.0 / (reach - n).max(1e-3)).clamp(0.7, 1.5);
            let mut v = ((raw - n) * gain).clamp(0.0, 1.0);
            if e.sealed {
                if let Some(native) = native {
                    if let (Some(jaw), Some(lips), Some(tongue)) = (
                        native.get("JawDrop"),
                        native.get("LipsToward"),
                        native.get("TongueOut"),
                    ) {
                        v *= 1.0 - ramp(jaw - lips, LIPS_APART).max(ramp(*tongue, TONGUE_OUT));
                    }
                }
            }
            post.push((name.clone(), v));
            let settled = if e.continuous {
                v < 0.5
            } else {
                !self.on[name] && v < e.on && !self.speaking
            };
            if at_rest(e.quiet) && settled && dt != 0.0 {
                let refreshed = n + (raw - n) * (dt / NEUTRAL_SECONDS).min(1.0);
                self.neutral.insert(name.clone(), refreshed.clamp(0.0, 0.5));
            }
        }

        let mut out: Vec<(String, f64)> = values.to_vec();
        let set = |out: &mut Vec<(String, f64)>, name: &str, value: f64| {
            if let Some(entry) = out.iter_mut().find(|(n, _)| n == name) {
                entry.1 = value;
            }
        };
        let seconds = now as f64 / 1e9;
        for (name, v) in &post {
            let e = config[name.as_str()];
            let mut v = *v;
            if e.continuous {
                let filtered = self.filters.get_mut(name).unwrap().update(v, seconds);
                set(&mut out, name, filtered);
                continue;
            }
            if let Some(exclusive) = e.exclusive {
                let rival = post
                    .iter()
                    .filter(|(k, _)| k.starts_with(exclusive))
                    .map(|&(_, w)| w)
                    .fold(-1.0, f64::max);
                if v <= rival {
                    v = 0.0;
                }
            }
            let raise = if self.speaking { e.speech_raise } else { 0.0 };
            let hold_on = if self.speaking {
                e.hold_on.max(e.speech_hold)
            } else {
                e.hold_on
            };
            let (on, off) = (e.on + raise + self.bias, e.off + raise + self.bias);
            let is_on = self.on[name];
            let changing = if is_on { v < off } else { v >= on };
            if !changing {
                self.since.insert(name.clone(), None);
            } else if self.since[name].is_none() {
                self.since.insert(name.clone(), Some(now));
            }
            if changing {
                let since = self.since[name].unwrap_or(now);
                let hold = if is_on { e.hold_off } else { hold_on };
                if (now - since) as f64 / 1e9 >= hold {
                    self.on.insert(name.clone(), !is_on);
                    self.since.insert(name.clone(), None);
                    if is_on {
                        self.filters.insert(name.clone(), OneEuro::face());
                    }
                }
            }
            let value = if self.on[name] {
                self.filters.get_mut(name).unwrap().update(v, seconds)
            } else {
                0.0
            };
            set(&mut out, name, value);
        }
        out
    }
}
