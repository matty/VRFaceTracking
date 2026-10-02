//! Debug: every value on its way from the tracking module to what's sent,
//! drawn as the chain of steps it passes through. Pick a step to see what it
//! changed, or a value to follow it through each one.
use crate::home::output_label;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    h_flex, v_flex, Disableable as _, Icon, Selectable as _, Sizable as _, StyledExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, relative, uniform_list, AnyElement, AppContext as _, ClipboardItem, Context, Div,
    Entity, Hsla, InteractiveElement as _, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement as _, Styled, Subscription, Task, UniformListScrollHandle, Window,
};
use rust_i18n::t;
use std::ops::Range;
use std::time::Duration;
use vrft_gui_core::client::{PipelineTrace, RunMode, StageKind, TraceParam};
use vrft_gui_core::launcher::{Launcher, StartVrft};
use vrft_gui_core::live::DaemonState;
use vrft_gui_core::nav::{open_page, PageId};
use vrft_gui_core::palette::{self, MONO_FONT};
use vrft_gui_core::summary::{Connection, Tone};
use vrft_gui_core::widgets::{cap, card, hint, ButtonExt as _, Notice, PageHeader};

const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// A change smaller than this share of a value's range counts as none.
const CHANGE_EPSILON: f32 = 0.001;
/// Polls a change stays counted for, so lists and counts don't flicker as
/// values settle: 1.5 seconds.
const HOLD_POLLS: u8 = 15;
const NODE_MIN_WIDTH: f32 = 118.;
const NODE_HEIGHT: f32 = 108.;
const CONNECTOR_WIDTH: f32 = 22.;
const ROW_HEIGHT: f32 = 30.;
/// The columns showing a value from the module and as sent.
const VALUE_COLUMN_WIDTH: f32 = 150.;
/// Each step's column of changes.
const STEP_COLUMN_WIDTH: f32 = 70.;
/// Narrowest content that shows each step's column.
const STEP_COLUMNS_MIN_WIDTH: f32 = 780.;
/// The page around the value list: header, chain, toolbar and padding.
const AROUND_LIST: f32 = 600.;
const MIN_LIST_HEIGHT: f32 = 300.;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Show {
    All,
    Changed,
}

/// A change of `delta` is more than noise for `param`.
fn moved(delta: f32, param: &TraceParam) -> bool {
    delta.abs() > CHANGE_EPSILON * span(param)
}

fn span(param: &TraceParam) -> f32 {
    (param.max - param.min).max(f32::EPSILON)
}

/// `value` as the list shows it: three decimals, two for wide ranges.
fn number(value: f32, param: &TraceParam) -> String {
    let decimals = if span(param) > 2. { 2 } else { 3 };
    // No `-0.000`.
    let value = if value.abs() < 0.5 * 10f32.powi(-decimals) {
        0.
    } else {
        value
    };
    format!("{value:.*}", decimals as usize)
}

fn signed(delta: f32, param: &TraceParam) -> String {
    let text = number(delta, param);
    if text.starts_with('-') {
        text.replacen('-', "\u{2212}", 1)
    } else {
        format!("+{text}")
    }
}

/// One frame's trace, with each step's changes worked out.
struct Snapshot {
    trace: PipelineTrace,
    /// For each stage: each value's change from the stage that ran before.
    /// Empty for the first, and for stages that didn't run.
    deltas: Vec<Vec<f32>>,
}

impl Snapshot {
    fn new(trace: PipelineTrace) -> Self {
        let count = trace.params.len();
        let mut deltas = Vec::with_capacity(trace.stages.len());
        let mut before: Option<&[f32]> = None;
        for stage in &trace.stages {
            let ran = stage.active && stage.values.len() == count;
            deltas.push(match (ran, before) {
                (true, Some(before)) => stage
                    .values
                    .iter()
                    .zip(before)
                    .map(|(after, before)| after - before)
                    .collect(),
                _ => Vec::new(),
            });
            if ran {
                before = Some(&stage.values);
            }
        }
        Self { trace, deltas }
    }

    fn ran(&self, stage: usize) -> bool {
        self.trace
            .stages
            .get(stage)
            .is_some_and(|stage| stage.active && stage.values.len() == self.trace.params.len())
    }

    /// Value `param` after stage `stage`, when it ran.
    fn value(&self, stage: usize, param: usize) -> Option<f32> {
        self.ran(stage)
            .then(|| self.trace.stages[stage].values[param])
    }

    fn delta(&self, stage: usize, param: usize) -> Option<f32> {
        self.deltas.get(stage)?.get(param).copied()
    }

    /// The first stage that ran: the module's values.
    fn first(&self) -> Option<usize> {
        (0..self.trace.stages.len()).find(|&stage| self.ran(stage))
    }

    /// The last stage that ran: what's sent.
    fn last(&self) -> Option<usize> {
        (0..self.trace.stages.len())
            .rev()
            .find(|&stage| self.ran(stage))
    }

    /// Stages after the first that ran, which change values.
    fn steps(&self) -> Vec<usize> {
        let first = self.first();
        (0..self.trace.stages.len())
            .filter(|&stage| self.ran(stage) && Some(stage) != first)
            .collect()
    }

    fn param_index(&self, name: &str) -> Option<usize> {
        self.trace
            .params
            .iter()
            .position(|param| param.name == name)
    }
}

/// Which values each stage changed lately, so counts and the list hold
/// still while values settle.
#[derive(Default)]
struct Recent {
    /// For each stage, each value's polls left to count as changed.
    held: Vec<Vec<u8>>,
}

impl Recent {
    fn update(&mut self, snapshot: &Snapshot) {
        let stages = snapshot.trace.stages.len();
        let params = snapshot.trace.params.len();
        if self.held.len() != stages || self.held.iter().any(|held| held.len() != params) {
            self.held = vec![vec![0; params]; stages];
        }
        for (stage, held) in self.held.iter_mut().enumerate() {
            for (index, polls) in held.iter_mut().enumerate() {
                let now = snapshot
                    .delta(stage, index)
                    .is_some_and(|delta| moved(delta, &snapshot.trace.params[index]));
                *polls = if now {
                    HOLD_POLLS
                } else {
                    polls.saturating_sub(1)
                };
            }
        }
    }

    fn changed(&self, stage: usize, param: usize) -> bool {
        self.held
            .get(stage)
            .and_then(|held| held.get(param))
            .is_some_and(|polls| *polls > 0)
    }

    fn count(&self, stage: usize) -> usize {
        self.held
            .get(stage)
            .map_or(0, |held| held.iter().filter(|polls| **polls > 0).count())
    }

    fn changed_anywhere(&self, param: usize) -> bool {
        (0..self.held.len()).any(|stage| self.changed(stage, param))
    }

    fn clear(&mut self) {
        self.held.clear();
    }
}

/// One box in the chain: a stage of the trace, or the output.
struct Node {
    /// Its stage in the trace; `None` for the output.
    stage: Option<usize>,
    kind: Option<StageKind>,
    label: SharedString,
    icon: IconName,
    active: bool,
}

pub struct DebugPage {
    daemon: Entity<DaemonState>,
    launcher: Entity<Launcher>,
    snapshot: Option<Snapshot>,
    recent: Recent,
    error: Option<String>,
    paused: bool,
    watching: bool,
    show: Show,
    search: Entity<InputState>,
    /// The search as last applied, lower case.
    query: String,
    /// The value being followed, by name.
    selected: Option<String>,
    /// The step whose changes the list shows, by its place in the trace.
    focus: Option<usize>,
    /// The values that pass the filters, by index.
    visible: Vec<usize>,
    scroll: UniformListScrollHandle,
    _poll: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl DebugPage {
    pub fn new(
        daemon: Entity<DaemonState>,
        launcher: Entity<Launcher>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("debug.search_placeholder")));
        let poll = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(POLL_INTERVAL).await;
            let Ok(client) = this.update(cx, |page, cx| {
                page.wants_frames(cx).then(|| page.daemon.read(cx).client())
            }) else {
                break;
            };
            let Some(client) = client else {
                continue;
            };
            let result = cx
                .background_executor()
                .spawn(async move { client.pipeline() })
                .await;
            if this.update(cx, |page, cx| page.apply(result, cx)).is_err() {
                break;
            }
        });
        let subscriptions = vec![cx.observe(&search, |page, _, cx| page.search_changed(cx))];
        Self {
            daemon,
            launcher,
            snapshot: None,
            recent: Recent::default(),
            error: None,
            paused: false,
            watching: false,
            show: Show::All,
            search,
            query: String::new(),
            selected: None,
            focus: None,
            visible: Vec::new(),
            scroll: UniformListScrollHandle::new(),
            _poll: poll,
            _subscriptions: subscriptions,
        }
    }

    pub fn set_watching(&mut self, watching: bool, cx: &mut Context<Self>) {
        self.watching = watching;
        cx.notify();
    }

    fn wants_frames(&self, cx: &Context<Self>) -> bool {
        self.watching && !self.paused && *self.daemon.read(cx).connection() == Connection::Online
    }

    fn apply(&mut self, result: anyhow::Result<Option<PipelineTrace>>, cx: &mut Context<Self>) {
        // Paused while the request was out.
        if self.paused {
            return;
        }
        match result {
            Ok(Some(trace)) => {
                let snapshot = Snapshot::new(trace);
                self.recent.update(&snapshot);
                if self
                    .focus
                    .is_some_and(|focus| focus >= snapshot.trace.stages.len())
                {
                    self.focus = None;
                }
                self.snapshot = Some(snapshot);
                self.error = None;
                self.refilter();
            }
            // Nothing recorded yet: the last frame shown stays.
            Ok(None) => self.error = None,
            Err(error) => self.error = Some(format!("{error:#}")),
        }
        cx.notify();
    }

    fn refilter(&mut self) {
        self.visible.clear();
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        for (index, param) in snapshot.trace.params.iter().enumerate() {
            if !self.query.is_empty()
                && !param.name.to_lowercase().contains(&self.query)
                && !param.group.contains(&self.query)
            {
                continue;
            }
            let shown = match self.focus {
                Some(stage) => self.recent.changed(stage, index),
                None => self.show == Show::All || self.recent.changed_anywhere(index),
            };
            if shown {
                self.visible.push(index);
            }
        }
    }

    fn search_changed(&mut self, cx: &mut Context<Self>) {
        let query = self.search.read(cx).value().trim().to_lowercase();
        if query != self.query {
            self.query = query;
            self.refilter();
            cx.notify();
        }
    }

    fn set_show(&mut self, show: Show, cx: &mut Context<Self>) {
        self.show = show;
        self.focus = None;
        self.refilter();
        cx.notify();
    }

    fn set_paused(&mut self, paused: bool, cx: &mut Context<Self>) {
        self.paused = paused;
        if !paused {
            // Changes counted before the pause are long over.
            self.recent.clear();
        }
        cx.notify();
    }

    fn toggle_focus(&mut self, stage: usize, cx: &mut Context<Self>) {
        self.focus = if self.focus == Some(stage) {
            None
        } else {
            Some(stage)
        };
        self.refilter();
        cx.notify();
    }

    fn select(&mut self, name: String, cx: &mut Context<Self>) {
        self.selected = if self.selected.as_deref() == Some(name.as_str()) {
            None
        } else {
            Some(name)
        };
        cx.notify();
    }

    /// The value being followed, by index.
    fn selected_index(&self) -> Option<usize> {
        let snapshot = self.snapshot.as_ref()?;
        snapshot.param_index(self.selected.as_deref()?)
    }

    /// Every value after each stage that ran, as tab-separated text.
    fn table_text(&self) -> String {
        let Some(snapshot) = &self.snapshot else {
            return String::new();
        };
        let stages: Vec<usize> = (0..snapshot.trace.stages.len())
            .filter(|&stage| snapshot.ran(stage))
            .collect();
        let mut text = std::iter::once("Value".to_string())
            .chain(
                stages
                    .iter()
                    .map(|&stage| snapshot.trace.stages[stage].name.clone()),
            )
            .collect::<Vec<_>>()
            .join("\t");
        for (index, param) in snapshot.trace.params.iter().enumerate() {
            text.push('\n');
            text.push_str(&param.name);
            for &stage in &stages {
                let value = snapshot.value(stage, index).unwrap_or_default();
                text.push_str(&format!("\t{value:.4}"));
            }
        }
        text
    }

    /// What to call a stage, and its icon.
    fn describe(
        &self,
        kind: StageKind,
        name: &str,
        cx: &Context<Self>,
    ) -> (SharedString, IconName) {
        match kind {
            StageKind::Module => {
                let module = self
                    .daemon
                    .read(cx)
                    .status()
                    .and_then(|status| status.daemon.as_ref())
                    .and_then(|daemon| daemon.module.as_ref())
                    .map(|module| module.display_name());
                (
                    module.unwrap_or_else(|| t!("debug.module").into()).into(),
                    IconName::Package,
                )
            }
            StageKind::Overrides => (t!("debug.overrides").into(), IconName::Pencil),
            StageKind::Adjustment => (t!("debug.adjustment").into(), IconName::SlidersHorizontal),
            StageKind::Correctors => (t!("debug.correctors").into(), IconName::WandSparkles),
            StageKind::Smoothing => (t!("debug.smoothing").into(), IconName::AudioWaveform),
            StageKind::Extension => (name.to_string().into(), IconName::Puzzle),
            StageKind::Other => (name.to_string().into(), IconName::Route),
        }
    }

    /// The chain's boxes: each stage, leaving out test values while there
    /// are none, then the output.
    fn nodes(&self, snapshot: &Snapshot, cx: &Context<Self>) -> Vec<Node> {
        let mut nodes: Vec<Node> = snapshot
            .trace
            .stages
            .iter()
            .enumerate()
            .filter(|(_, stage)| stage.kind != StageKind::Overrides || stage.active)
            .map(|(index, stage)| {
                let (label, icon) = self.describe(stage.kind, &stage.name, cx);
                Node {
                    stage: Some(index),
                    kind: Some(stage.kind),
                    label,
                    icon,
                    active: snapshot.ran(index),
                }
            })
            .collect();
        let output = self
            .daemon
            .read(cx)
            .status()
            .and_then(|status| status.daemon.as_ref())
            .and_then(|daemon| daemon.output.as_ref())
            .map(|output| output_label(&output.mode));
        nodes.push(Node {
            stage: None,
            kind: None,
            label: output.unwrap_or_else(|| t!("debug.output").into()).into(),
            icon: IconName::Send,
            active: true,
        });
        nodes
    }

    /// The page that changes a node's stage.
    fn node_page(node: &Node) -> Option<PageId> {
        match node.kind {
            None => Some(PageId::SETTINGS),
            Some(StageKind::Module) => Some(PageId::MODULES),
            Some(StageKind::Adjustment | StageKind::Correctors | StageKind::Smoothing) => {
                Some(PageId::TRACKING)
            }
            Some(_) => None,
        }
    }

    fn node(&self, node: &Node, snapshot: &Snapshot, cx: &Context<Self>) -> AnyElement {
        let selected = self.selected_index();
        let first = snapshot.first();
        let last = snapshot.last();
        // The output shows what the last step that ran left.
        let shows = node.stage.or(last);
        let is_step = node.stage.is_some() && node.stage != first;
        let focused = is_step && node.stage == self.focus;
        let rate = self.daemon.read(cx).rates().tracking_fps;

        let delta = selected.zip(node.stage).and_then(|(param, stage)| {
            let delta = snapshot.delta(stage, param)?;
            Some((delta, moved(delta, &snapshot.trace.params[param])))
        });
        let lit = node.active
            && match (selected, node.stage) {
                (Some(_), Some(_)) => delta.is_some_and(|(_, moved)| moved),
                (None, Some(stage)) if is_step => self.recent.count(stage) > 0,
                _ => true,
            };

        let body: AnyElement = if !node.active {
            div()
                .text_size(px(12.))
                .text_color(palette::text_4())
                .child(t!("debug.off"))
                .into_any_element()
        } else if let (Some(param), Some(stage)) = (selected, shows) {
            let info = &snapshot.trace.params[param];
            let value = snapshot.value(stage, param).unwrap_or_default();
            let change = match delta {
                Some((delta, true)) => signed(delta, info),
                Some((_, false)) => t!("debug.no_change").into(),
                None if node.stage == first && !snapshot.trace.fresh => t!("debug.held").into(),
                None => String::new(),
            };
            v_flex()
                .gap_1p5()
                .child(
                    div()
                        .font_family(MONO_FONT)
                        .text_size(px(18.))
                        .font_medium()
                        .text_color(palette::text())
                        .child(number(value, info)),
                )
                .child(value_bar(value, info, palette::text()))
                .child(
                    div()
                        .font_family(MONO_FONT)
                        .text_size(px(11.5))
                        .text_color(if delta.is_some_and(|(_, moved)| moved) {
                            palette::text()
                        } else {
                            palette::text_3()
                        })
                        .child(change),
                )
                .into_any_element()
        } else {
            let (reading, detail): (String, Option<String>) = match node.stage {
                None => (
                    rate.map_or_else(
                        || t!("debug.sent").into(),
                        |rate| t!("debug.rate", rate = format!("{rate:.0}")).into(),
                    ),
                    None,
                ),
                Some(stage) if Some(stage) == first => (
                    t!("debug.values_count", count = snapshot.trace.params.len()).into(),
                    (!snapshot.trace.fresh).then(|| t!("debug.held").into()),
                ),
                Some(stage) => match self.recent.count(stage) {
                    0 => (t!("debug.no_change").into(), None),
                    count => (t!("debug.changed_count", count = count).into(), None),
                },
            };
            v_flex()
                .gap_1()
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(if lit {
                            palette::text()
                        } else {
                            palette::text_3()
                        })
                        .child(reading),
                )
                .children(detail.map(|detail| {
                    div()
                        .text_size(px(11.5))
                        .text_color(palette::text_3())
                        .child(detail)
                }))
                .into_any_element()
        };

        let id = SharedString::from(format!(
            "debug-node-{}",
            node.stage
                .map_or("output".to_string(), |stage| stage.to_string())
        ));
        let page = Self::node_page(node);
        let tooltip: Option<SharedString> = if is_step && node.active {
            Some(t!("debug.step_tooltip").into())
        } else {
            page.filter(|_| !is_step)
                .map(|page| t!("debug.open_page", page = page.label()).into())
        };
        let stage = node.stage;
        let active = node.active;
        v_flex()
            .id(id)
            .flex_1()
            .min_w(px(NODE_MIN_WIDTH))
            .h(px(NODE_HEIGHT))
            .gap_2()
            .p(px(11.))
            .rounded(px(11.))
            .border_1()
            .when(!node.active, |node| node.border_dashed())
            .border_color(if focused {
                palette::text_2()
            } else if lit {
                palette::line_focus()
            } else {
                palette::line_strong()
            })
            .bg(if !node.active {
                palette::sunken()
            } else if focused {
                palette::raised()
            } else {
                palette::inset()
            })
            .when(tooltip.is_some(), |node| {
                node.cursor_pointer()
                    .hover(|style| style.border_color(palette::text_3()))
            })
            .when_some(tooltip, |node, tooltip| {
                node.tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
            })
            .on_click(cx.listener(move |page_view, _, _, cx| match stage {
                Some(stage) if is_step => {
                    if active {
                        page_view.toggle_focus(stage, cx)
                    }
                }
                _ => {
                    if let Some(page) = page {
                        open_page(page, cx);
                    }
                }
            }))
            .child(
                h_flex()
                    .gap_2()
                    .min_w_0()
                    .text_color(if node.active {
                        palette::text_2()
                    } else {
                        palette::text_4()
                    })
                    .child(Icon::new(node.icon).size(px(14.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.5))
                            .font_medium()
                            .text_color(if node.active {
                                palette::text()
                            } else {
                                palette::text_3()
                            })
                            .child(node.label.clone()),
                    ),
            )
            .child(body)
            .into_any_element()
    }

    /// The chain of steps, each joined to the next.
    fn chain(&self, snapshot: &Snapshot, cx: &Context<Self>) -> AnyElement {
        let nodes = self.nodes(snapshot, cx);
        let mut row = h_flex().w_full().items_center();
        for (index, node) in nodes.iter().enumerate() {
            if index > 0 {
                row = row.child(connector(node.active));
            }
            row = row.child(self.node(node, snapshot, cx));
        }
        div()
            .id("debug-chain")
            .w_full()
            .overflow_x_scroll()
            .pb_1()
            .child(row)
            .into_any_element()
    }

    /// The line under the chain: what the list shows, and how to change it.
    fn chain_footer(&self, snapshot: &Snapshot, cx: &Context<Self>) -> Div {
        let row = h_flex().gap_3().min_h(px(28.)).text_size(px(12.5));
        if let Some(stage) = self.focus.filter(|&stage| snapshot.ran(stage)) {
            let trace_stage = &snapshot.trace.stages[stage];
            let (label, _) = self.describe(trace_stage.kind, &trace_stage.name, cx);
            let page = match trace_stage.kind {
                StageKind::Adjustment | StageKind::Correctors | StageKind::Smoothing => {
                    Some(PageId::TRACKING)
                }
                _ => None,
            };
            return row
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(palette::text_2())
                        .child(t!("debug.showing_step", step = label)),
                )
                .children(page.map(|page| {
                    Button::new("debug-open-step")
                        .ghost()
                        .small()
                        .label(t!("debug.open_page", page = page.label()))
                        .on_click(move |_, _, cx| open_page(page, cx))
                }))
                .child(
                    Button::new("debug-show-all")
                        .ghost()
                        .small()
                        .label(t!("debug.show_all"))
                        .on_click(cx.listener(|page, _, _, cx| {
                            page.focus = None;
                            page.refilter();
                            cx.notify();
                        })),
                );
        }
        if let Some(name) = self
            .selected
            .clone()
            .filter(|_| self.selected_index().is_some())
        {
            return row
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(palette::text_2())
                        .child(t!("debug.following", name = name)),
                )
                .child(
                    Button::new("debug-stop-following")
                        .ghost()
                        .small()
                        .label(t!("debug.stop_following"))
                        .on_click(cx.listener(|page, _, _, cx| {
                            page.selected = None;
                            cx.notify();
                        })),
                );
        }
        row.child(
            div()
                .text_color(palette::text_3())
                .child(t!("debug.chain_hint")),
        )
    }

    /// How the frames stand: live, paused, or the module gone quiet.
    fn state_line(&self, snapshot: &Snapshot) -> AnyElement {
        let (tone, text): (Tone, SharedString) = if self.paused {
            (Tone::Off, t!("debug.paused").into())
        } else if !snapshot.trace.fresh {
            (Tone::Waiting, t!("debug.quiet").into())
        } else {
            (Tone::Good, t!("debug.live").into())
        };
        h_flex()
            .gap_2()
            .text_size(px(12.))
            .text_color(if tone == Tone::Good {
                palette::text_2()
            } else {
                palette::text_3()
            })
            .child(vrft_gui_core::widgets::StatusDot::new(tone))
            .child(text)
            .into_any_element()
    }

    fn segments(&self, cx: &Context<Self>) -> Div {
        let choices = [
            (Show::All, SharedString::from(t!("debug.all"))),
            (Show::Changed, SharedString::from(t!("debug.changed"))),
        ];
        h_flex()
            .flex_none()
            .gap(px(2.))
            .p(px(3.))
            .rounded(px(9.))
            .bg(palette::sunken())
            .border_1()
            .border_color(palette::line())
            .children(
                choices
                    .into_iter()
                    .enumerate()
                    .map(|(index, (choice, label))| {
                        let on = self.focus.is_none() && choice == self.show;
                        Button::new(("debug-show", index))
                            .ghost()
                            .small()
                            .h_7()
                            .px_3()
                            .rounded(px(6.))
                            .label(label)
                            .selected(on)
                            .map(|segment| {
                                if on {
                                    segment
                                        .bg(Hsla::from(gpui_kit::rgb(0x26262b)))
                                        .text_color(palette::text())
                                } else {
                                    segment.text_color(palette::text_2())
                                }
                            })
                            .on_click(cx.listener(move |page, _, _, cx| page.set_show(choice, cx)))
                    }),
            )
    }

    fn toolbar(&self, snapshot: &Snapshot, cx: &Context<Self>) -> Div {
        h_flex()
            .flex_wrap()
            .gap_2()
            .items_center()
            .child(self.segments(cx))
            .child(
                div().flex_1().min_w(px(160.)).child(
                    Input::new(&self.search)
                        .small()
                        .cleanable(true)
                        .prefix(
                            div()
                                .text_color(palette::text_3())
                                .child(Icon::new(IconName::Search).size(px(14.))),
                        )
                        .bg(palette::sunken()),
                ),
            )
            .child(
                div()
                    .flex_none()
                    .font_family(MONO_FONT)
                    .text_size(px(11.5))
                    .text_color(palette::text_3())
                    .child(t!(
                        "debug.shown",
                        shown = self.visible.len(),
                        total = snapshot.trace.params.len()
                    )),
            )
    }

    fn header_row(&self, snapshot: &Snapshot, steps: &[usize], cx: &Context<Self>) -> Div {
        let column = |label: SharedString, width: f32, focused: bool| {
            div()
                .w(px(width))
                .flex_none()
                .truncate()
                .text_right()
                .when(focused, |cell| cell.text_color(palette::text()))
                .child(label)
        };
        h_flex()
            .h(px(28.))
            .px_3()
            .gap_3()
            .font_family(MONO_FONT)
            .text_size(px(10.5))
            .text_color(palette::text_3())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(SharedString::from(t!("debug.column_value").to_uppercase())),
            )
            .child(column(
                t!("debug.column_module").to_uppercase().into(),
                VALUE_COLUMN_WIDTH,
                false,
            ))
            .children(steps.iter().map(|&stage| {
                let trace_stage = &snapshot.trace.stages[stage];
                let (label, _) = self.describe(trace_stage.kind, &trace_stage.name, cx);
                column(
                    label.to_uppercase().into(),
                    STEP_COLUMN_WIDTH,
                    self.focus == Some(stage),
                )
            }))
            .child(column(
                t!("debug.column_sent").to_uppercase().into(),
                VALUE_COLUMN_WIDTH,
                false,
            ))
    }

    fn row(&self, index: usize, steps: &[usize], cx: &Context<Self>) -> AnyElement {
        let Some(snapshot) = &self.snapshot else {
            return div().h(px(ROW_HEIGHT)).into_any_element();
        };
        let param = &snapshot.trace.params[index];
        let selected = self.selected.as_deref() == Some(param.name.as_str());
        let raw = snapshot
            .first()
            .and_then(|stage| snapshot.value(stage, index));
        let sent = snapshot
            .last()
            .and_then(|stage| snapshot.value(stage, index));
        let changed = self.recent.changed_anywhere(index);
        let name = param.name.clone();
        h_flex()
            .id(("debug-value", index))
            .h(px(ROW_HEIGHT))
            .w_full()
            .px_3()
            .gap_3()
            .cursor_pointer()
            .font_family(MONO_FONT)
            .text_size(px(12.))
            .when(selected, |row| row.bg(palette::raised()))
            .when(!selected, |row| {
                row.hover(|style| style.bg(palette::inset()))
            })
            .on_click(cx.listener(move |page, _, _, cx| page.select(name.clone(), cx)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(if changed || selected {
                        palette::text()
                    } else {
                        palette::text_2()
                    })
                    .child(param.name.clone()),
            )
            .child(value_cell(raw, param, palette::text_3(), palette::text_2()))
            .children(steps.iter().map(|&stage| {
                let delta = snapshot.delta(stage, index);
                let focused = self.focus == Some(stage);
                let (text, color) = match delta {
                    Some(delta) if moved(delta, param) => (signed(delta, param), palette::text()),
                    Some(_) if self.recent.changed(stage, index) => {
                        ("~".to_string(), palette::text_3())
                    }
                    _ => ("\u{00b7}".to_string(), palette::text_4()),
                };
                div()
                    .w(px(STEP_COLUMN_WIDTH))
                    .h_full()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_end()
                    .text_size(px(11.5))
                    .text_color(color)
                    .when(focused, |cell| cell.bg(palette::inset()))
                    .child(text)
            }))
            .child(value_cell(sent, param, palette::text(), palette::text()))
            .into_any_element()
    }

    fn list(&self, snapshot: &Snapshot, height: f32, wide: bool, cx: &Context<Self>) -> Div {
        let steps: Vec<usize> = if wide { snapshot.steps() } else { Vec::new() };
        let frame = div()
            .h(px(height))
            .rounded(px(10.))
            .border_1()
            .border_color(palette::line())
            .bg(palette::sunken())
            .overflow_hidden();
        let header = self.header_row(snapshot, &steps, cx);
        if self.visible.is_empty() {
            let empty = if self.focus.is_some() || self.show == Show::Changed {
                t!("debug.nothing_changed")
            } else {
                t!("debug.no_match")
            };
            return frame.child(header).child(
                div()
                    .flex()
                    .flex_1()
                    .h(px(height - 60.))
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(palette::text_3())
                    .child(empty),
            );
        }
        frame.flex().flex_col().child(header).child(
            div().flex_1().min_h_0().child(
                uniform_list(
                    "debug-values",
                    self.visible.len(),
                    cx.processor(move |page, range: Range<usize>, _, cx| {
                        range
                            .filter_map(|row| page.visible.get(row).copied())
                            .map(|index| page.row(index, &steps, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.scroll)
                .size_full(),
            ),
        )
    }
}

/// A line from one box to the next, faint into a step that's off.
fn connector(into_active: bool) -> impl IntoElement {
    let color = if into_active {
        palette::text_3()
    } else {
        palette::text_4()
    };
    h_flex()
        .w(px(CONNECTOR_WIDTH))
        .flex_none()
        .items_center()
        .child(div().flex_1().h(px(1.)).bg(color))
        .child(
            div()
                .flex_none()
                .ml(px(-5.))
                .text_color(color)
                .child(Icon::new(IconName::ChevronRight).size(px(12.))),
        )
}

/// A thin bar for `value` across its range. A range either side of zero
/// fills from zero, marked on the bar.
fn value_bar(value: f32, param: &TraceParam, color: Hsla) -> Div {
    let at = ((value - param.min) / span(param)).clamp(0., 1.);
    let zero = ((0. - param.min) / span(param)).clamp(0., 1.);
    let (start, end) = if zero <= at { (zero, at) } else { (at, zero) };
    div()
        .relative()
        .w_full()
        .h(px(4.))
        .rounded_full()
        .bg(palette::line_strong())
        .child(
            div()
                .absolute()
                .top_0()
                .left(relative(start))
                .w(relative(end - start))
                .h_full()
                .rounded_full()
                .bg(color),
        )
        .when(param.min < 0., |bar| {
            bar.child(
                div()
                    .absolute()
                    .top(px(-2.))
                    .left(relative(zero))
                    .w(px(1.))
                    .h(px(8.))
                    .bg(palette::text_3()),
            )
        })
}

/// A value as a bar and a number.
fn value_cell(value: Option<f32>, param: &TraceParam, bar: Hsla, text: Hsla) -> Div {
    h_flex()
        .w(px(VALUE_COLUMN_WIDTH))
        .flex_none()
        .gap_2()
        .child(
            div()
                .flex_1()
                .children(value.map(|value| value_bar(value, param, bar))),
        )
        .child(
            div()
                .w(px(56.))
                .flex_none()
                .text_right()
                .text_color(text)
                .child(value.map_or_else(|| "\u{2014}".to_string(), |value| number(value, param))),
        )
}

impl Render for DebugPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.daemon.read(cx);
        let online = *state.connection() == Connection::Online;
        let extensions_only = state
            .status()
            .and_then(|status| status.daemon.as_ref())
            .is_some_and(|daemon| daemon.mode == RunMode::ExtensionsOnly);
        let paused = self.paused;
        let header = PageHeader::new(t!("debug.title"))
            .description(t!("debug.description"))
            .trailing(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("debug-copy")
                            .ghost()
                            .regular()
                            .icon(IconName::Copy)
                            .label(t!("debug.copy"))
                            .tooltip(t!("debug.copy_hint"))
                            .disabled(self.snapshot.is_none())
                            .on_click(cx.listener(|page, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(page.table_text()))
                            })),
                    )
                    .child(
                        Button::new("debug-pause")
                            .ghost()
                            .regular()
                            .icon(if paused {
                                IconName::Play
                            } else {
                                IconName::Pause
                            })
                            .label(if paused {
                                t!("debug.resume")
                            } else {
                                t!("debug.pause")
                            })
                            .disabled(self.snapshot.is_none())
                            .on_click(
                                cx.listener(move |page, _, _, cx| page.set_paused(!paused, cx)),
                            ),
                    ),
            );
        let page = v_flex().gap_6().child(header);
        if !online {
            return page
                .child(
                    v_flex()
                        .gap_3()
                        .items_start()
                        .child(hint(t!("debug.start_to_see"), cx))
                        .child(StartVrft::new(self.launcher.clone())),
                )
                .into_any_element();
        }
        if extensions_only {
            return page
                .child(hint(t!("debug.extensions_only"), cx))
                .into_any_element();
        }
        let error = self.error.clone();
        let Some(snapshot) = &self.snapshot else {
            return page
                .children(error.map(|error| Notice::new(Tone::Problem, error)))
                .child(hint(t!("debug.waiting"), cx))
                .into_any_element();
        };
        let wide = vrft_gui_core::content_width(window) >= STEP_COLUMNS_MIN_WIDTH;
        let height = (window.viewport_size().height.as_f32() - AROUND_LIST).max(MIN_LIST_HEIGHT);
        page.children(error.map(|error| Notice::new(Tone::Problem, error)))
            .child(
                card(cx)
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        h_flex()
                            .justify_between()
                            .child(cap(t!("debug.pipeline")))
                            .child(self.state_line(snapshot)),
                    )
                    .child(self.chain(snapshot, cx))
                    .child(self.chain_footer(snapshot, cx)),
            )
            .child(
                card(cx)
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(self.toolbar(snapshot, cx))
                    .child(self.list(snapshot, height, wide, cx)),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vrft_gui_core::client::TraceStage;

    fn param(name: &str, min: f32) -> TraceParam {
        TraceParam {
            name: name.into(),
            group: "jaw".into(),
            min,
            max: 1.,
        }
    }

    fn stage(kind: StageKind, values: &[f32]) -> TraceStage {
        TraceStage {
            kind,
            name: format!("{kind:?}"),
            active: !values.is_empty(),
            values: values.to_vec(),
        }
    }

    fn snapshot() -> Snapshot {
        Snapshot::new(PipelineTrace {
            fresh: true,
            params: vec![param("JawOpen", 0.), param("HeadYaw", -1.)],
            stages: vec![
                stage(StageKind::Module, &[0.5, 0.2]),
                stage(StageKind::Overrides, &[]),
                stage(StageKind::Correctors, &[0.4, 0.2]),
                stage(StageKind::Smoothing, &[0.45, 0.1]),
            ],
        })
    }

    #[test]
    fn each_step_is_compared_with_the_last_that_ran() {
        let snapshot = snapshot();
        assert_eq!(snapshot.first(), Some(0));
        assert_eq!(snapshot.last(), Some(3));
        assert_eq!(snapshot.steps(), [2, 3]);
        // Correctors follows the module, past the test values that are off.
        assert!((snapshot.delta(2, 0).unwrap() + 0.1).abs() < 1e-6);
        assert_eq!(snapshot.delta(2, 1), Some(0.));
        assert!((snapshot.delta(3, 1).unwrap() + 0.1).abs() < 1e-6);
        assert_eq!(snapshot.delta(1, 0), None);
    }

    #[test]
    fn changes_are_held_for_a_while() {
        let snapshot = snapshot();
        let mut recent = Recent::default();
        recent.update(&snapshot);
        assert!(recent.changed(2, 0));
        assert!(!recent.changed(2, 1));
        assert_eq!(recent.count(3), 2);
        assert!(recent.changed_anywhere(1));

        let still = Snapshot::new(PipelineTrace {
            stages: vec![
                stage(StageKind::Module, &[0.5, 0.2]),
                stage(StageKind::Overrides, &[]),
                stage(StageKind::Correctors, &[0.5, 0.2]),
                stage(StageKind::Smoothing, &[0.5, 0.2]),
            ],
            ..snapshot.trace.clone()
        });
        for _ in 0..HOLD_POLLS - 1 {
            recent.update(&still);
        }
        assert!(recent.changed(2, 0));
        recent.update(&still);
        assert!(!recent.changed(2, 0));
    }

    #[test]
    fn numbers_read_cleanly() {
        let shape = param("JawOpen", 0.);
        assert_eq!(number(0.74219, &shape), "0.742");
        assert_eq!(number(-0.0001, &shape), "0.000");
        assert_eq!(signed(0.031, &shape), "+0.031");
        assert_eq!(signed(-0.031, &shape), "\u{2212}0.031");
        let pupil = TraceParam {
            max: 10.,
            ..param("EyeLeftPupil", 0.)
        };
        assert_eq!(number(3.256, &pupil), "3.26");
    }
}
