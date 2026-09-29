//! Tracking settings: how VRFT tunes tracking between the module and VRChat
//! (smoothing, VRCFaceTracking's corrections and its expression ranges), as
//! saved in the daemon's `config.json`. Like Settings, changes save as
//! they're made and apply when VRFT next starts.
use crate::settings::{changed_mark, field, field_label, group, restart_banner, GROUPS_WIDE_MIN};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::InputState;
use gpui_kit::component::slider::{Slider, SliderState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{h_flex, v_flex, Disableable as _, Sizable as _, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, AnyElement, AppContext as _, Context, Div, Entity, IntoElement, ParentElement, Render,
    SharedString, Styled, Subscription, Task, Window,
};
use rust_i18n::t;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;
use vrft_gui_core::client::{
    AdjustmentGroup, AdjustmentTuning, Config, ConfigPatch, CorrectorsTuning, FilterTuning, Tuning,
};
use vrft_gui_core::launcher::{Launcher, StartVrft};
use vrft_gui_core::live::DaemonState;
use vrft_gui_core::palette::{self, MONO_FONT};
use vrft_gui_core::summary::{self, Connection, Fix, Tone};
use vrft_gui_core::widgets::{
    cap, card, divider, fix_button, hint, ButtonExt as _, Notice, PageHeader,
};

const CHECK_INTERVAL: Duration = Duration::from_millis(250);
/// What the range sliders' handles snap to.
const RANGE_STEP: f32 = 0.01;
/// The derivative cutoff when its field is left empty.
const DEFAULT_D_CUTOFF: f32 = 0.1;
/// The column naming each expression range.
const RANGE_LABEL_WIDTH: f32 = 210.;
/// The column showing each expression range's values.
const RANGE_VALUE_WIDTH: f32 = 100.;
/// How far a slider's thumb reaches past the ends of its track.
const THUMB_OVERHANG: f32 = 8.;

/// `value` to two decimals, as the range sliders step.
fn round2(value: f32) -> f32 {
    (value * 100.).round() / 100.
}

/// A number typed into a smoothing field: `None` when it's left empty.
/// `zero_allowed` says whether 0 is a value it can take.
fn parse_number(text: &str, name: &str, zero_allowed: bool) -> Result<Option<f32>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    text.parse::<f32>()
        .ok()
        .filter(|value| value.is_finite() && (*value > 0. || (zero_allowed && *value == 0.)))
        .map(Some)
        .ok_or_else(|| {
            if zero_allowed {
                t!(
                    "tracking.not_a_number_zero_or_more",
                    text = text,
                    name = name
                )
            } else {
                t!("tracking.not_a_number_above_zero", text = text, name = name)
            }
            .into()
        })
}

fn min_cutoff(text: &str) -> Result<Option<f32>, String> {
    parse_number(text, &t!("tracking.minimum_cutoff_name"), false)
}

fn beta(text: &str) -> Result<Option<f32>, String> {
    parse_number(text, &t!("tracking.beta_name"), true)
}

fn d_cutoff(text: &str) -> Result<f32, String> {
    Ok(
        parse_number(text, &t!("tracking.derivative_cutoff_name"), false)?
            .unwrap_or(DEFAULT_D_CUTOFF),
    )
}

/// A setting typed into a field differs from `saved`, or isn't a value.
fn typed_differs<T: PartialEq>(typed: Result<T, String>, saved: T) -> bool {
    typed.map_or(true, |typed| typed != saved)
}

/// `range` isn't `other`, beyond slider rounding.
fn range_differs(range: [f32; 2], other: [f32; 2]) -> bool {
    (range[0] - other[0]).abs() > 0.001 || (range[1] - other[1]).abs() > 0.001
}

fn full_range(group: &AdjustmentGroup) -> [f32; 2] {
    [group.min, group.max]
}

/// The tuning as set on the page.
#[derive(Debug, Clone, PartialEq)]
struct Form {
    /// `mutator.enabled`: any tuning at all.
    enabled: bool,
    /// 0 to 100.
    smoothing: f32,
    head: bool,
    min_cutoff: String,
    beta: String,
    d_cutoff: String,
    correctors: bool,
    mouth_closed_clamp: bool,
    lip_suck_limiter: bool,
    eye_look_symmetrize: bool,
    /// 0 to 100.
    eyelid_blend: f32,
    adjustment: bool,
    /// Every group's `[floor, ceil]`, full where it's not set.
    ranges: BTreeMap<String, [f32; 2]>,
}

/// Which of the form's settings differ from a config.
#[derive(Debug, Default, Clone, PartialEq)]
struct Changes {
    enabled: bool,
    smoothing: bool,
    head: bool,
    min_cutoff: bool,
    beta: bool,
    d_cutoff: bool,
    correctors: bool,
    mouth_closed_clamp: bool,
    lip_suck_limiter: bool,
    eye_look_symmetrize: bool,
    eyelid_blend: bool,
    adjustment: bool,
    /// The groups whose range changed.
    ranges: BTreeSet<String>,
}

impl Changes {
    fn any(&self) -> bool {
        self.enabled
            || self.smoothing
            || self.head
            || self.min_cutoff
            || self.beta
            || self.d_cutoff
            || self.correctors
            || self.mouth_closed_clamp
            || self.lip_suck_limiter
            || self.eye_look_symmetrize
            || self.eyelid_blend
            || self.adjustment
            || !self.ranges.is_empty()
    }
}

impl Form {
    fn from_config(config: &Config) -> Self {
        let tuning = &config.tuning;
        let number = |value: Option<f32>| value.map(|v| format!("{v}")).unwrap_or_default();
        Self {
            enabled: config.smoothing_enabled,
            smoothing: (config.smoothing * 100.).round(),
            head: tuning.filter.head,
            min_cutoff: number(tuning.filter.min_cutoff),
            beta: number(tuning.filter.beta),
            d_cutoff: format!("{}", tuning.filter.d_cutoff),
            correctors: tuning.correctors.enabled,
            mouth_closed_clamp: tuning.correctors.mouth_closed_clamp,
            lip_suck_limiter: tuning.correctors.lip_suck_limiter,
            eye_look_symmetrize: tuning.correctors.eye_look_symmetrize,
            eyelid_blend: (tuning.correctors.eyelid_blend * 100.).round(),
            adjustment: tuning.adjustment.enabled,
            ranges: config
                .adjustment_groups
                .iter()
                .map(|group| {
                    let range = tuning
                        .adjustment
                        .ranges
                        .get(&group.key)
                        .copied()
                        .unwrap_or(full_range(group));
                    (group.key.clone(), range)
                })
                .collect(),
        }
    }

    /// Which settings saving the form would change in `config`. Text that
    /// isn't a number counts as a change, so saving says what's wrong with
    /// it.
    fn changes(&self, config: &Config) -> Changes {
        let saved = Self::from_config(config);
        let filter = &config.tuning.filter;
        Changes {
            enabled: self.enabled != saved.enabled,
            smoothing: self.smoothing.round() != saved.smoothing,
            head: self.head != saved.head,
            min_cutoff: typed_differs(min_cutoff(&self.min_cutoff), filter.min_cutoff),
            beta: typed_differs(beta(&self.beta), filter.beta),
            d_cutoff: typed_differs(d_cutoff(&self.d_cutoff), filter.d_cutoff),
            correctors: self.correctors != saved.correctors,
            mouth_closed_clamp: self.mouth_closed_clamp != saved.mouth_closed_clamp,
            lip_suck_limiter: self.lip_suck_limiter != saved.lip_suck_limiter,
            eye_look_symmetrize: self.eye_look_symmetrize != saved.eye_look_symmetrize,
            eyelid_blend: self.eyelid_blend.round() != saved.eyelid_blend,
            adjustment: self.adjustment != saved.adjustment,
            ranges: self
                .ranges
                .iter()
                .filter(|(key, range)| {
                    saved
                        .ranges
                        .get(*key)
                        .is_none_or(|saved| range_differs(**range, *saved))
                })
                .map(|(key, _)| key.clone())
                .collect(),
        }
    }

    /// The form as a change to `config`, or why it can't be one. A VRFT too
    /// old for tuning only gets the smoothing.
    fn patch(&self, config: &Config) -> Result<ConfigPatch, String> {
        let tuning = if config.adjustment_groups.is_empty() {
            None
        } else {
            let full: BTreeMap<&str, [f32; 2]> = config
                .adjustment_groups
                .iter()
                .map(|group| (group.key.as_str(), full_range(group)))
                .collect();
            Some(Tuning {
                filter: FilterTuning {
                    min_cutoff: min_cutoff(&self.min_cutoff)?,
                    beta: beta(&self.beta)?,
                    d_cutoff: d_cutoff(&self.d_cutoff)?,
                    head: self.head,
                },
                correctors: CorrectorsTuning {
                    enabled: self.correctors,
                    mouth_closed_clamp: self.mouth_closed_clamp,
                    lip_suck_limiter: self.lip_suck_limiter,
                    eyelid_blend: self.eyelid_blend.round() / 100.,
                    eye_look_symmetrize: self.eye_look_symmetrize,
                },
                adjustment: AdjustmentTuning {
                    enabled: self.adjustment,
                    ranges: self
                        .ranges
                        .iter()
                        .filter(|(key, range)| {
                            full.get(key.as_str())
                                .is_some_and(|full| range_differs(**range, *full))
                        })
                        .map(|(key, [floor, ceil])| (key.clone(), [round2(*floor), round2(*ceil)]))
                        .collect(),
                },
            })
        };
        Ok(ConfigPatch {
            smoothing_enabled: Some(self.enabled),
            smoothing: Some(self.smoothing.round() / 100.),
            tuning,
            ..ConfigPatch::default()
        })
    }
}

/// One expression range: its group and the slider setting it.
struct RangeSlider {
    group: AdjustmentGroup,
    slider: Entity<SliderState>,
    _observe: Subscription,
}

pub struct TrackingPage {
    daemon: Entity<DaemonState>,
    launcher: Entity<Launcher>,
    /// The settings as last read from or saved to VRFT.
    config: Option<Config>,
    /// The settings VRFT is running with: as first read since it started.
    applied: Option<Config>,
    enabled: bool,
    head: bool,
    correctors: bool,
    mouth_closed_clamp: bool,
    lip_suck_limiter: bool,
    eye_look_symmetrize: bool,
    adjustment: bool,
    smoothing: Entity<SliderState>,
    eyelid_blend: Entity<SliderState>,
    min_cutoff: Entity<InputState>,
    beta: Entity<InputState>,
    d_cutoff: Entity<InputState>,
    /// In the order VRFT gives the groups.
    ranges: Vec<RangeSlider>,
    /// The page at the last check; it saves once it's the same twice.
    settled: Option<Form>,
    /// A form that failed to save, so it isn't tried again.
    failed: Option<Form>,
    /// Why the page as set can't be saved.
    invalid: Option<String>,
    /// Why reading or saving failed.
    message: Option<Notice>,
    saving: bool,
    watching: bool,
    /// Read the settings again at the next chance.
    stale: bool,
    _poll: Task<()>,
    _save: Option<Task<()>>,
    _subscriptions: [Subscription; 7],
}

impl TrackingPage {
    pub fn new(
        daemon: Entity<DaemonState>,
        launcher: Entity<Launcher>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let percent = || SliderState::new().min(0.).max(100.).step(1.);
        let smoothing = cx.new(|_| percent());
        let eyelid_blend = cx.new(|_| percent());
        let min_cutoff = cx.new(|cx| InputState::new(window, cx).placeholder(t!("tracking.auto")));
        let beta = cx.new(|cx| InputState::new(window, cx).placeholder(t!("tracking.auto")));
        let d_cutoff = cx.new(|cx| InputState::new(window, cx).placeholder("0.1"));
        let client = daemon.read(cx).client();
        // Reads the settings while the page shows and they're stale, such as
        // after VRFT restarts.
        let poll = cx.spawn_in(window, async move |this, cx| loop {
            let Ok(read) = this.update(cx, |page, cx| {
                page.watching
                    && page.stale
                    && *page.daemon.read(cx).connection() == Connection::Online
            }) else {
                break;
            };
            if read {
                let client = client.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move { client.config() })
                    .await;
                if this
                    .update_in(cx, |page, window, cx| page.loaded(result, window, cx))
                    .is_err()
                {
                    break;
                }
            }
            if this.update(cx, |page, cx| page.autosave(cx)).is_err() {
                break;
            }
            cx.background_executor().timer(CHECK_INTERVAL).await;
        });
        let subscriptions = [
            cx.observe(&daemon, |page, daemon, cx| {
                // A restart applies what was saved; read it again.
                if *daemon.read(cx).connection() != Connection::Online {
                    page.stale = true;
                    page.applied = None;
                }
                cx.notify();
            }),
            cx.observe(&launcher, |_, _, cx| cx.notify()),
            // Moving and typing change what's different from what VRFT runs.
            cx.observe(&smoothing, |_, _, cx| cx.notify()),
            cx.observe(&eyelid_blend, |_, _, cx| cx.notify()),
            cx.observe(&min_cutoff, |_, _, cx| cx.notify()),
            cx.observe(&beta, |_, _, cx| cx.notify()),
            cx.observe(&d_cutoff, |_, _, cx| cx.notify()),
        ];
        Self {
            daemon,
            launcher,
            config: None,
            applied: None,
            enabled: true,
            head: true,
            correctors: true,
            mouth_closed_clamp: true,
            lip_suck_limiter: true,
            eye_look_symmetrize: false,
            adjustment: false,
            smoothing,
            eyelid_blend,
            min_cutoff,
            beta,
            d_cutoff,
            ranges: Vec::new(),
            settled: None,
            failed: None,
            invalid: None,
            message: None,
            saving: false,
            watching: false,
            stale: true,
            _poll: poll,
            _save: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn set_watching(&mut self, watching: bool, cx: &mut Context<Self>) {
        self.watching = watching;
        if watching {
            self.stale = true;
        }
        cx.notify();
    }

    fn loaded(
        &mut self,
        result: anyhow::Result<Config>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stale = false;
        match result {
            Ok(config) => {
                if self.applied.is_none() {
                    self.applied = Some(config.clone());
                }
                // Changes yet to save aren't thrown away by a re-read.
                let unsaved = self
                    .config
                    .as_ref()
                    .is_some_and(|old| self.form(cx).changes(old).any());
                if unsaved {
                    self.config = Some(config);
                } else {
                    self.fill(config, window, cx);
                }
            }
            Err(error) => self.message = Some(Notice::error(&t!("tracking.read_failed"), &error)),
        }
        cx.notify();
    }

    /// A slider for each of `config`'s expression ranges, made again only
    /// when the groups change.
    fn make_ranges(&mut self, config: &Config, cx: &mut Context<Self>) {
        let same = self.ranges.len() == config.adjustment_groups.len()
            && self
                .ranges
                .iter()
                .zip(&config.adjustment_groups)
                .all(|(range, group)| range.group == *group);
        if same {
            return;
        }
        self.ranges = config
            .adjustment_groups
            .iter()
            .map(|group| {
                let slider = cx.new(|_| {
                    SliderState::new()
                        .min(group.min)
                        .max(group.max)
                        .step(RANGE_STEP)
                        .default_value((group.min, group.max))
                });
                RangeSlider {
                    group: group.clone(),
                    _observe: cx.observe(&slider, |_, _, cx| cx.notify()),
                    slider,
                }
            })
            .collect();
    }

    /// Shows `config` on the page.
    fn fill(&mut self, config: Config, window: &mut Window, cx: &mut Context<Self>) {
        self.make_ranges(&config, cx);
        self.show(Form::from_config(&config), window, cx);
        self.config = Some(config);
    }

    /// Puts `form` in the page's controls.
    fn show(&mut self, form: Form, window: &mut Window, cx: &mut Context<Self>) {
        self.enabled = form.enabled;
        self.head = form.head;
        self.correctors = form.correctors;
        self.mouth_closed_clamp = form.mouth_closed_clamp;
        self.lip_suck_limiter = form.lip_suck_limiter;
        self.eye_look_symmetrize = form.eye_look_symmetrize;
        self.adjustment = form.adjustment;
        let texts = [
            (&self.min_cutoff, form.min_cutoff),
            (&self.beta, form.beta),
            (&self.d_cutoff, form.d_cutoff),
        ];
        for (input, text) in texts {
            input.update(cx, |input, cx| input.set_value(text, window, cx));
        }
        for (slider, value) in [
            (&self.smoothing, form.smoothing),
            (&self.eyelid_blend, form.eyelid_blend),
        ] {
            slider.update(cx, |slider, cx| slider.set_value(value, window, cx));
        }
        for range in &self.ranges {
            let [floor, ceil] = form
                .ranges
                .get(&range.group.key)
                .copied()
                .unwrap_or(full_range(&range.group));
            range
                .slider
                .update(cx, |slider, cx| slider.set_value((floor, ceil), window, cx));
        }
    }

    /// The page's settings as set.
    fn form(&self, cx: &Context<Self>) -> Form {
        let text = |input: &Entity<InputState>| input.read(cx).value().to_string();
        Form {
            enabled: self.enabled,
            smoothing: self.smoothing.read(cx).value().start(),
            head: self.head,
            min_cutoff: text(&self.min_cutoff),
            beta: text(&self.beta),
            d_cutoff: text(&self.d_cutoff),
            correctors: self.correctors,
            mouth_closed_clamp: self.mouth_closed_clamp,
            lip_suck_limiter: self.lip_suck_limiter,
            eye_look_symmetrize: self.eye_look_symmetrize,
            eyelid_blend: self.eyelid_blend.read(cx).value().start(),
            adjustment: self.adjustment,
            ranges: self
                .ranges
                .iter()
                .map(|range| {
                    let value = range.slider.read(cx).value();
                    (
                        range.group.key.clone(),
                        [round2(value.start()), round2(value.end())],
                    )
                })
                .collect(),
        }
    }

    /// Saves the page once it's stopped changing, so a slider or typing
    /// saves when it settles rather than at every step.
    fn autosave(&mut self, cx: &mut Context<Self>) {
        let Some(config) = self.config.as_ref() else {
            return;
        };
        if self.saving {
            return;
        }
        let form = self.form(cx);
        if self.settled.as_ref() != Some(&form) {
            self.settled = Some(form);
            return;
        }
        if !form.changes(config).any() || self.failed.as_ref() == Some(&form) {
            if self.invalid.take().is_some() {
                cx.notify();
            }
            return;
        }
        match form.patch(config) {
            Ok(_) => {
                self.invalid = None;
                self.save(false, cx);
            }
            Err(reason) => {
                if self.invalid.as_ref() != Some(&reason) {
                    self.invalid = Some(reason);
                    cx.notify();
                }
            }
        }
    }

    /// Puts back the settings VRFT is running with, which then save.
    fn revert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(applied) = self.applied.clone() {
            self.show(Form::from_config(&applied), window, cx);
        }
        self.failed = None;
        self.message = None;
        cx.notify();
    }

    /// Restarts VRFT so it uses what's saved, saving anything yet to save
    /// first.
    fn restart(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let unsaved = self
            .config
            .as_ref()
            .is_some_and(|config| self.form(cx).changes(config).any());
        if unsaved {
            self.save(true, cx);
        } else {
            self.launcher
                .update(cx, |launcher, cx| launcher.restart(cx));
        }
    }

    /// Saves the page, then restarts VRFT so it applies when `restart`.
    fn save(&mut self, restart: bool, cx: &mut Context<Self>) {
        let Some(config) = self.config.clone() else {
            return;
        };
        let form = self.form(cx);
        let patch = match form.patch(&config) {
            Ok(patch) => patch,
            Err(reason) => {
                self.invalid = Some(reason);
                cx.notify();
                return;
            }
        };
        self.saving = true;
        self.message = None;
        cx.notify();
        let client = self.daemon.read(cx).client();
        self._save = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { client.update_config(&patch) })
                .await;
            this.update(cx, |page, cx| {
                page.saving = false;
                match result {
                    Ok(config) => {
                        page.failed = None;
                        page.config = Some(config);
                        if restart {
                            page.stale = true;
                            page.launcher
                                .update(cx, |launcher, cx| launcher.restart(cx));
                        }
                    }
                    Err(error) => {
                        page.failed = Some(form);
                        page.message = Some(Notice::error(&t!("tracking.save_failed"), &error));
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Puts expression range `index` back to its full range.
    fn reset_range(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(range) = self.ranges.get(index) {
            let full = (range.group.min, range.group.max);
            range
                .slider
                .update(cx, |slider, cx| slider.set_value(full, window, cx));
        }
        cx.notify();
    }

    /// A setting with a switch: its name, marked when changed, and what it
    /// does under it.
    #[allow(clippy::too_many_arguments)]
    fn switch_row(
        &self,
        id: &'static str,
        title: impl Into<SharedString>,
        description: impl Into<SharedString>,
        checked: bool,
        changed: bool,
        disabled: bool,
        set: fn(&mut Self, bool),
        cx: &Context<Self>,
    ) -> Div {
        let title: SharedString = title.into();
        h_flex()
            .gap_3()
            .items_center()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(setting_name(title.clone(), changed))
                    .child(hint(description, cx)),
            )
            .child(
                Switch::new(id)
                    .checked(checked)
                    .disabled(disabled)
                    .accessibility_label(title)
                    .on_change(cx.listener(move |page, on: &bool, _, cx| {
                        set(page, *on);
                        cx.notify();
                    })),
            )
    }

    fn tuning_card(&self, changes: &Changes, cx: &Context<Self>) -> Div {
        card(cx).p(px(18.)).child(self.switch_row(
            "tuning-enabled",
            t!("tracking.tune_tracking"),
            t!("tracking.tune_tracking_hint"),
            self.enabled,
            changes.enabled,
            false,
            |page, on| page.enabled = on,
            cx,
        ))
    }

    fn smoothing_card(&self, changes: &Changes, supported: bool, cx: &Context<Self>) -> Div {
        let off = !self.enabled;
        let amount = self.smoothing.read(cx).value().start().round();
        card(cx)
            .p(px(18.))
            .flex()
            .flex_col()
            .gap_4()
            .child(
                v_flex()
                    .gap_3()
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(setting_name(t!("tracking.amount"), changes.smoothing).flex_1())
                            .child(reading(if off {
                                t!("tracking.off").into()
                            } else {
                                format!("{amount:.0}%")
                            })),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(slider(&self.smoothing, off, false))
                            .child(
                                h_flex()
                                    .justify_between()
                                    .text_size(px(11.5))
                                    .text_color(palette::text_3())
                                    .child(t!("tracking.quicker"))
                                    .child(t!("tracking.steadier")),
                            ),
                    ),
            )
            .when(supported, |card| {
                card.child(self.switch_row(
                    "smooth-head",
                    t!("tracking.smooth_head"),
                    t!("tracking.smooth_head_hint"),
                    self.head,
                    changes.head,
                    off,
                    |page, on| page.head = on,
                    cx,
                ))
                .child(divider(cx))
                .child(
                    v_flex()
                        .gap_3()
                        .child(
                            v_flex()
                                .gap_0p5()
                                .child(setting_name(t!("tracking.fine_tuning"), false))
                                .child(hint(t!("tracking.fine_tuning_hint"), cx)),
                        )
                        .child(
                            h_flex()
                                .gap_3()
                                .flex_wrap()
                                .child(number_field(
                                    t!("tracking.minimum_cutoff"),
                                    &self.min_cutoff,
                                    changes.min_cutoff,
                                ))
                                .child(number_field(t!("tracking.beta"), &self.beta, changes.beta))
                                .child(number_field(
                                    t!("tracking.derivative_cutoff"),
                                    &self.d_cutoff,
                                    changes.d_cutoff,
                                )),
                        ),
                )
            })
    }

    fn corrections_card(&self, changes: &Changes, cx: &Context<Self>) -> Div {
        let off = !self.enabled || !self.correctors;
        let blend = self.eyelid_blend.read(cx).value().start().round();
        card(cx)
            .p(px(18.))
            .flex()
            .flex_col()
            .gap_4()
            .child(self.switch_row(
                "correctors-enabled",
                t!("tracking.corrections"),
                t!("tracking.corrections_hint"),
                self.correctors,
                changes.correctors,
                !self.enabled,
                |page, on| page.correctors = on,
                cx,
            ))
            .child(divider(cx))
            .child(self.switch_row(
                "mouth-closed-clamp",
                t!("tracking.mouth_closed_clamp"),
                t!("tracking.mouth_closed_clamp_hint"),
                self.mouth_closed_clamp,
                changes.mouth_closed_clamp,
                off,
                |page, on| page.mouth_closed_clamp = on,
                cx,
            ))
            .child(self.switch_row(
                "lip-suck-limiter",
                t!("tracking.lip_suck_limiter"),
                t!("tracking.lip_suck_limiter_hint"),
                self.lip_suck_limiter,
                changes.lip_suck_limiter,
                off,
                |page, on| page.lip_suck_limiter = on,
                cx,
            ))
            .child(self.switch_row(
                "eye-look-symmetrize",
                t!("tracking.eye_look_symmetrize"),
                t!("tracking.eye_look_symmetrize_hint"),
                self.eye_look_symmetrize,
                changes.eye_look_symmetrize,
                off,
                |page, on| page.eye_look_symmetrize = on,
                cx,
            ))
            .child(
                v_flex()
                    .gap_3()
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_0p5()
                                    .child(setting_name(
                                        t!("tracking.eyelid_blend"),
                                        changes.eyelid_blend,
                                    ))
                                    .child(hint(t!("tracking.eyelid_blend_hint"), cx)),
                            )
                            .child(reading(format!("{blend:.0}%"))),
                    )
                    .child(slider(&self.eyelid_blend, off, false)),
            )
    }

    fn ranges_card(&self, changes: &Changes, cx: &Context<Self>) -> Div {
        let off = !self.enabled || !self.adjustment;
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut in_head = false;
        for (index, range) in self.ranges.iter().enumerate() {
            let head = range.group.min < 0.;
            if index == 0 || head != in_head {
                in_head = head;
                rows.push(
                    div()
                        .when(index > 0, |label| label.pt_2())
                        .child(cap(if head {
                            t!("tracking.head")
                        } else {
                            t!("tracking.face")
                        }))
                        .into_any_element(),
                );
            }
            rows.push(self.range_row(index, range, changes, off, cx));
        }
        card(cx)
            .p(px(18.))
            .flex()
            .flex_col()
            .gap_4()
            .child(self.switch_row(
                "adjustment-enabled",
                t!("tracking.expression_ranges"),
                t!("tracking.expression_ranges_hint"),
                self.adjustment,
                changes.adjustment,
                !self.enabled,
                |page, on| page.adjustment = on,
                cx,
            ))
            .child(divider(cx))
            .child(v_flex().gap_2().children(rows))
    }

    fn range_row(
        &self,
        index: usize,
        range: &RangeSlider,
        changes: &Changes,
        off: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let value = range.slider.read(cx).value();
        let (floor, ceil) = (round2(value.start()), round2(value.end()));
        let changed = changes.ranges.contains(&range.group.key);
        let full = !range_differs([floor, ceil], full_range(&range.group));
        h_flex()
            .gap_3()
            .items_center()
            .min_h(px(30.))
            .child(
                h_flex()
                    .w(px(RANGE_LABEL_WIDTH))
                    .flex_none()
                    .gap_1p5()
                    .text_size(px(12.5))
                    .text_color(if off {
                        palette::text_3()
                    } else {
                        palette::text_2()
                    })
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .child(SharedString::from(range.group.label.clone())),
                    )
                    .when(changed, |label| label.child(changed_mark()))
                    .when(!full, |label| {
                        label.child(
                            Button::new(("reset-range", index))
                                .ghost()
                                .xsmall()
                                .icon(IconName::RotateCcw)
                                .tooltip(t!("tracking.use_full_range"))
                                .disabled(off)
                                .on_click(cx.listener(move |page, _, window, cx| {
                                    page.reset_range(index, window, cx)
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(80.))
                    .child(slider(&range.slider, off, true)),
            )
            .child(
                div()
                    .w(px(RANGE_VALUE_WIDTH))
                    .flex_none()
                    .text_right()
                    .font_family(MONO_FONT)
                    .text_size(px(12.))
                    .text_color(if full {
                        palette::text_3()
                    } else {
                        palette::text_2()
                    })
                    .child(format!("{floor:.2} \u{2013} {ceil:.2}")),
            )
            .into_any_element()
    }
}

/// A slider inset so its thumbs line up with the card's content at either
/// end. Its bar dims while it's `off`, as a disabled slider hides its
/// thumbs; a range's bar is grey, so a page of them isn't a column of white.
fn slider(state: &Entity<SliderState>, off: bool, range: bool) -> Div {
    let bar = if off {
        palette::text_4()
    } else if range {
        palette::text_2()
    } else {
        palette::text()
    };
    div()
        .px(px(THUMB_OVERHANG))
        .child(Slider::new(state).disabled(off).bg(bar))
}

/// A setting's name, marked when it's changed.
fn setting_name(title: impl Into<SharedString>, changed: bool) -> Div {
    h_flex()
        .gap_1p5()
        .text_size(px(13.5))
        .font_medium()
        .child(title.into())
        .when(changed, |label| label.child(changed_mark()))
}

/// A setting's value beside its name.
fn reading(text: String) -> Div {
    div()
        .font_family(MONO_FONT)
        .text_size(px(12.5))
        .text_color(palette::text_2())
        .child(text)
}

/// A small number field with its name above it.
fn number_field(label: impl Into<SharedString>, state: &Entity<InputState>, changed: bool) -> Div {
    v_flex()
        .w(px(160.))
        .flex_none()
        .gap_1p5()
        .child(field_label(label, changed))
        .child(field(state, changed))
}

impl Render for TrackingPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.daemon.read(cx);
        let online = *state.connection() == Connection::Online;
        let config_error = state
            .status()
            .and_then(|status| status.daemon.as_ref())
            .and_then(|daemon| daemon.config_error.clone());
        let header = PageHeader::new(t!("tracking.title"))
            .description(t!("tracking.description"))
            .trailing(
                fix_button("open-config", Fix::Config)
                    .ghost()
                    .regular()
                    .icon(IconName::FileText),
            );
        let page = v_flex()
            .gap_6()
            .child(header)
            .when_some(config_error, |page, error| {
                page.child(Notice::new(Tone::Problem, summary::config_error(&error)))
            });
        if !online {
            return page
                .child(
                    v_flex()
                        .gap_3()
                        .items_start()
                        .child(hint(t!("tracking.start_to_change"), cx))
                        .child(StartVrft::new(self.launcher.clone())),
                )
                .into_any_element();
        }
        let Some(config) = self.config.clone() else {
            return page
                .child(hint(t!("tracking.reading"), cx))
                .children(self.message.clone())
                .into_any_element();
        };
        let wide = vrft_gui_core::content_width(window) >= GROUPS_WIDE_MIN;
        let supported = !config.adjustment_groups.is_empty();
        let applied = self.applied.clone().unwrap_or_else(|| config.clone());
        let changes = self.form(cx).changes(&applied);
        let pending = Form::from_config(&config).changes(&applied).any();
        let restarting = self.launcher.read(cx).is_busy();
        page.when(pending, |page| {
            page.child(restart_banner(
                self.saving || restarting,
                restarting,
                cx,
                Self::revert,
                Self::restart,
            ))
        })
        .children(
            self.invalid
                .clone()
                .map(|reason| Notice::new(Tone::Problem, reason)),
        )
        .children(self.message.clone())
        .child(
            v_flex()
                .gap_6()
                .child(group(
                    t!("tracking.tuning_group"),
                    t!("tracking.tuning_group_hint"),
                    self.tuning_card(&changes, cx),
                    wide,
                ))
                .child(group(
                    t!("tracking.smoothing_group"),
                    t!("tracking.smoothing_group_hint"),
                    self.smoothing_card(&changes, supported, cx),
                    wide,
                ))
                .when(!supported, |groups| {
                    groups.child(Notice::new(Tone::Problem, t!("tracking.too_old")))
                })
                .when(supported, |groups| {
                    groups
                        .child(group(
                            t!("tracking.corrections_group"),
                            t!("tracking.corrections_group_hint"),
                            self.corrections_card(&changes, cx),
                            wide,
                        ))
                        .child(group(
                            t!("tracking.expression_ranges_group"),
                            t!("tracking.expression_ranges_group_hint"),
                            self.ranges_card(&changes, cx),
                            wide,
                        ))
                }),
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        let groups = [
            ("jaw", "Jaw", 0.),
            ("eye_wide", "Eye Wide", 0.),
            ("head_yaw", "Yaw", -1.),
        ];
        let mut config = Config {
            smoothing_enabled: true,
            smoothing: 0.3,
            adjustment_groups: groups
                .iter()
                .map(|(key, label, min)| AdjustmentGroup {
                    key: (*key).into(),
                    label: (*label).into(),
                    min: *min,
                    max: 1.,
                })
                .collect(),
            ..Config::default()
        };
        config
            .tuning
            .adjustment
            .ranges
            .insert("jaw".into(), [0., 0.8]);
        config
    }

    #[test]
    fn an_untouched_form_has_nothing_to_save() {
        let config = config();
        let form = Form::from_config(&config);
        assert_eq!(form.ranges["jaw"], [0., 0.8]);
        assert_eq!(form.ranges["head_yaw"], [-1., 1.]);
        assert!(!form.changes(&config).any());
        // Numbers written differently aren't changes.
        let respelled = Form {
            d_cutoff: " 0.10 ".into(),
            ..form
        };
        assert!(!respelled.changes(&config).any());
    }

    #[test]
    fn changes_name_what_changed() {
        let config = config();
        let form = Form::from_config(&config);
        let mut moved = form.clone();
        moved.ranges.insert("eye_wide".into(), [0.1, 1.]);
        moved.eyelid_blend = 40.;
        let changes = moved.changes(&config);
        assert_eq!(changes.ranges, BTreeSet::from(["eye_wide".to_string()]));
        assert!(changes.eyelid_blend);
        assert!(!changes.smoothing);

        let typo = Form {
            beta: "fast".into(),
            ..form
        };
        assert!(typo.changes(&config).beta);
        assert!(typo.patch(&config).unwrap_err().contains("beta"));
    }

    #[test]
    fn the_patch_holds_only_ranges_that_change_something() {
        let config = config();
        let mut form = Form::from_config(&config);
        form.ranges.insert("eye_wide".into(), [0.1, 1.]);
        form.min_cutoff = "1.5".into();
        form.eyelid_blend = 25.;
        let patch = form.patch(&config).unwrap();
        assert_eq!(patch.smoothing, Some(0.3));
        assert_eq!(patch.smoothing_enabled, Some(true));
        let tuning = patch.tuning.unwrap();
        assert_eq!(
            tuning.adjustment.ranges,
            BTreeMap::from([("eye_wide".into(), [0.1, 1.]), ("jaw".into(), [0., 0.8])])
        );
        assert_eq!(tuning.filter.min_cutoff, Some(1.5));
        assert_eq!(tuning.filter.beta, None);
        assert_eq!(tuning.filter.d_cutoff, 0.1);
        assert_eq!(tuning.correctors.eyelid_blend, 0.25);
    }

    #[test]
    fn an_old_vrft_only_gets_the_smoothing() {
        let config = Config {
            adjustment_groups: Vec::new(),
            ..config()
        };
        let patch = Form::from_config(&config).patch(&config).unwrap();
        assert_eq!(patch.tuning, None);
        assert_eq!(patch.smoothing, Some(0.3));
    }

    #[test]
    fn filter_fields_take_only_usable_numbers() {
        assert_eq!(min_cutoff(""), Ok(None));
        assert_eq!(min_cutoff("2"), Ok(Some(2.)));
        assert!(min_cutoff("0").is_err());
        assert_eq!(beta("0"), Ok(Some(0.)));
        assert!(beta("-1").is_err());
        assert_eq!(d_cutoff(""), Ok(DEFAULT_D_CUTOFF));
        assert!(d_cutoff("nan").is_err());
    }
}
