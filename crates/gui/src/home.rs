//! Home: one card that answers "is it working?" with VRFT's controls beside
//! the answer and the signal path under it, then each extension's tiles.
//! Until tracking has been live, the answer is a short setup checklist.
use crate::shell::Extensions;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    h_flex, v_flex, ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    canvas, div, point, px, AnyElement, App, Bounds, ClipboardItem, Context, Entity, Hsla,
    InteractiveElement as _, IntoElement, ParentElement, PathBuilder, Pixels, Render, SharedString,
    StatefulInteractiveElement as _, Styled, Subscription, Window,
};
use rust_i18n::t;
use vrft_gui_core::client::{OutputTarget, Status};
use vrft_gui_core::extension::{open_page, PageId, PathSource};
use vrft_gui_core::launcher::{LaunchState, Launcher, StartVrft};
use vrft_gui_core::live::DaemonState;
use vrft_gui_core::palette::{self, MONO_FONT};
use vrft_gui_core::summary::{self, Connection, Fix, Rates, Tone};
use vrft_gui_core::widgets::{
    card, fix_button, ButtonExt as _, Notice, Section, StatTile, StatusDot,
};

pub struct HomePage {
    daemon: Entity<DaemonState>,
    launcher: Entity<Launcher>,
    extensions: Entity<Extensions>,
    /// Stop was pressed while tracking was live, and waits for a second yes.
    confirm_stop: bool,
    /// The whole of a failure's technical detail shows, not just its start.
    details_open: bool,
    /// Tracking has been live since the app started, so setup is done.
    ever_live: bool,
    _subscriptions: [Subscription; 3],
}

/// Narrowest a tile gets before the grid drops a column.
const MIN_TILE_WIDTH: f32 = 200.;
const TILE_GAP: f32 = 12.;
/// A stage of the signal path.
const NODE_HEIGHT: f32 = 56.;
const NODE_GAP: f32 = 10.;
/// The curves joining the sources to smoothing, and smoothing to output.
const JOIN_WIDTH: f32 = 80.;
const LINK_WIDTH: f32 = 64.;
/// The hero card's side padding, which the rule above the path runs into.
const HERO_PADDING: f32 = 32.;

impl HomePage {
    pub fn new(
        daemon: Entity<DaemonState>,
        launcher: Entity<Launcher>,
        extensions: Entity<Extensions>,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = [
            cx.observe(&daemon, |home, daemon, cx| {
                if daemon.read(cx).rates().tracking() {
                    home.ever_live = true;
                }
                cx.notify();
            }),
            cx.observe(&launcher, |_, _, cx| cx.notify()),
            cx.observe(&extensions, |_, _, cx| cx.notify()),
        ];
        Self {
            daemon,
            launcher,
            extensions,
            confirm_stop: false,
            details_open: false,
            ever_live: false,
            _subscriptions: subscriptions,
        }
    }

    /// Restart and Stop while VRFT runs, at the start of the status band.
    /// Restart is left out beside a problem's fix.
    fn engine_controls(&self, with_restart: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = self.daemon.read(cx);
        let online = *state.connection() == Connection::Online;
        let running = online && state.status().is_some_and(|status| status.daemon.is_some());
        let tracking = state.rates().tracking();
        let launcher = self.launcher.read(cx);
        let stuck = launcher.is_stuck();
        let busy = launcher.is_busy();
        let stopping = *launcher.state() == LaunchState::Stopping;
        let shown = engine_buttons(running, stuck, with_restart, self.confirm_stop);
        // It didn't stop when asked.
        let end = shown.end.then(|| {
            let launcher = self.launcher.clone();
            Button::new("end-vrft")
                .danger()
                .outline()
                .regular()
                .label(t!("home.end_vrft"))
                .on_click(move |_, _, cx| {
                    launcher.update(cx, |launcher, cx| launcher.end_stuck(false, cx))
                })
        });
        if !shown.stop {
            return end.map(IntoElement::into_any_element);
        }
        let restart = {
            let launcher = self.launcher.clone();
            Button::new("restart-vrft")
                .regular()
                .icon(IconName::RotateCcw)
                .label(t!("home.restart"))
                .tooltip(t!("home.restart_tooltip"))
                .disabled(busy)
                .on_click(move |_, _, cx| launcher.update(cx, |launcher, cx| launcher.restart(cx)))
        };
        let stop = if self.confirm_stop {
            h_flex()
                .gap_2()
                .child(
                    Button::new("stop-vrft")
                        .danger()
                        .outline()
                        .regular()
                        .label(t!("home.stop_tracking"))
                        .loading(stopping)
                        .on_click(cx.listener(|home, _, _, cx| {
                            home.confirm_stop = false;
                            home.launcher.update(cx, |launcher, cx| launcher.stop(cx));
                        })),
                )
                .child(
                    Button::new("keep-tracking")
                        .ghost()
                        .regular()
                        .label(t!("home.keep_tracking"))
                        .on_click(cx.listener(|home, _, _, cx| {
                            home.confirm_stop = false;
                            cx.notify();
                        })),
                )
                .into_any_element()
        } else {
            Button::new("stop-vrft")
                .regular()
                .icon(IconName::Power)
                .label(t!("home.stop"))
                .loading(stopping)
                .disabled(busy && !stopping)
                .on_click(cx.listener(move |home, _, _, cx| {
                    if tracking {
                        home.confirm_stop = true;
                        cx.notify();
                    } else {
                        home.launcher.update(cx, |launcher, cx| launcher.stop(cx));
                    }
                }))
                .into_any_element()
        };
        Some(
            h_flex()
                .gap_2()
                .when(shown.restart, |row| row.child(restart))
                .child(stop)
                .children(end)
                .into_any_element(),
        )
    }

    /// A failure's technical detail on one line, whole on a click, with a
    /// button that copies it.
    fn details_row(&self, detail: String, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.details_open;
        let copy = detail.clone();
        h_flex()
            .id("failure-details")
            .flex_1()
            .min_w_0()
            .gap_2p5()
            .items_start()
            .min_h(px(40.))
            .py(px(9.))
            .pl_3()
            .pr_1p5()
            .rounded(px(9.))
            .bg(palette::sunken())
            .border_1()
            .border_color(palette::line())
            .cursor_pointer()
            .on_click(cx.listener(|home, _, _, cx| {
                home.details_open = !home.details_open;
                cx.notify();
            }))
            .child(
                div()
                    .flex_none()
                    .pt(px(2.))
                    .text_color(palette::text_3())
                    .child(
                        Icon::new(if open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .size(px(14.)),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(12.5))
                    .text_color(palette::text_2())
                    .child(t!("home.details")),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .pt(px(1.))
                    .font_family(MONO_FONT)
                    .text_size(px(11.5))
                    .text_color(palette::text_3())
                    .when(!open, |text| text.truncate())
                    .child(detail),
            )
            .child(
                Button::new("copy-failure-details")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Copy)
                    .tooltip(t!("home.copy_details"))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                    }),
            )
    }

    /// The setup checklist shown until tracking has been live: choose a
    /// module, check where tracking goes, then put the headset on.
    fn checklist(&self, status: Option<&Status>) -> impl IntoElement {
        let report = status.and_then(|status| status.daemon.as_ref());
        let module = report.and_then(|report| report.module.as_ref());
        let module_name = module
            .map(|module| module.display_name())
            .unwrap_or_else(|| t!("home.module_not_chosen").into());
        let module_done = module.is_some_and(|module| module.loaded);
        let output = report.and_then(|report| report.output.as_ref());
        let (where_to, output_done) = match output {
            Some(output) => where_tracking_goes(output),
            None => (t!("home.output_unknown").into(), false),
        };
        let headset = match module {
            Some(module) => t!("home.headset_step", app = module.display_name()),
            None => t!("home.headset_step_no_module"),
        };
        v_flex()
            .child(checklist_row(
                t!("home.tracking_module"),
                module_name,
                module_done,
                Some(("change-module", PageId::MODULES)),
            ))
            .child(checklist_row(
                t!("home.where_tracking_goes"),
                where_to,
                output_done,
                Some(("change-output", PageId::SETTINGS)),
            ))
            .child(
                h_flex()
                    .items_start()
                    .gap(px(14.))
                    .pt_4()
                    .child(step_mark(false, true))
                    .child(
                        div()
                            .w(px(200.))
                            .flex_none()
                            .text_sm()
                            .font_semibold()
                            .child(t!("home.put_headset_on")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(13.))
                            .text_color(palette::text_2())
                            .child(headset),
                    ),
            )
    }

    fn signal_path(&self, status: Option<&Status>, rates: &Rates, cx: &App) -> impl IntoElement {
        let report = status.and_then(|status| status.daemon.as_ref());
        let tracking = rates.tracking();
        let module = summary::module(status, rates);
        let module_label = report
            .and_then(|report| report.module.as_ref())
            .map(|module| module.display_name())
            .unwrap_or_else(|| t!("home.tracking_module").into());
        let mut sources = vec![PathSource {
            icon: IconName::Package,
            label: module_label,
            value: rates.tracking_fps.filter(|_| tracking).map(summary::fps),
            tone: module.tone,
            page: PageId::MODULES,
        }];
        {
            let daemon = self.daemon.read(cx);
            let extensions = self.extensions.read(cx);
            sources.extend(
                extensions
                    .list
                    .iter()
                    .filter(|extension| extensions.shown(extension.id(), daemon))
                    .filter_map(|extension| extension.path_source(cx)),
            );
        }
        let flows: Vec<bool> = sources
            .iter()
            .map(|source| source.tone == Tone::Good)
            .collect();
        let output = report.and_then(|report| report.output.as_ref());
        let smoothing = output.map(|output| match output.smoothing {
            Some(amount) => format!("{:.0}%", amount * 100.),
            None => t!("home.smoothing_off").into(),
        });
        let downstream = if tracking { Tone::Good } else { Tone::Off };
        let count = sources.len();
        let column_height = count as f32 * NODE_HEIGHT + (count as f32 - 1.) * NODE_GAP;
        h_flex()
            .items_center()
            .child(v_flex().w(px(280.)).flex_none().gap(px(NODE_GAP)).children(
                sources.into_iter().enumerate().map(|(index, source)| {
                    path_node(
                        ("source", index),
                        source.icon,
                        source.label,
                        source.value,
                        source.tone,
                        source.page,
                    )
                    .into_any_element()
                }),
            ))
            .child(join(flows, tracking, column_height))
            .child(div().w(px(196.)).flex_none().child(path_node(
                ("stage", 0),
                IconName::SlidersHorizontal,
                t!("home.smoothing").into(),
                smoothing,
                // Its value says it all while tracking.
                if tracking { Tone::Good } else { Tone::Off },
                PageId::TRACKING,
            )))
            .child(link(tracking))
            .child(
                div().flex_1().min_w_0().child(path_node(
                    ("stage", 1),
                    IconName::Send,
                    output.map_or(t!("home.output").into(), |output| {
                        output_label(&output.mode)
                    }),
                    rates
                        .tracking_fps
                        .filter(|_| tracking)
                        .map(|rate| t!("home.rate_per_second", rate = format!("{rate:.0}")).into()),
                    downstream,
                    PageId::SETTINGS,
                )),
            )
    }

    /// Each extension's tiles, under its name.
    fn extension_sections(&self, available: f32, cx: &App) -> Vec<AnyElement> {
        let daemon = self.daemon.read(cx);
        let extensions = self.extensions.read(cx);
        extensions
            .list
            .iter()
            .filter(|extension| extensions.shown(extension.id(), daemon))
            .map(|extension| {
                (
                    extension.name(),
                    extension.description(),
                    extension.home_tiles(cx),
                )
            })
            .filter(|(_, _, tiles)| !tiles.is_empty())
            .map(|(name, description, tiles)| {
                Section::new(t!("home.add_on_section", name = name))
                    .aside(description)
                    .child(tile_grid(tiles, available))
                    .into_any_element()
            })
            .collect()
    }
}

impl Render for HomePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.daemon.read(cx);
        let status = state.status().cloned();
        let rates = state.rates();
        let connection = state.connection().clone();
        let online = connection == Connection::Online;
        let launch = self.launcher.read(cx).state().clone();
        let activity = self.launcher.read(cx).activity();
        let log = self.launcher.read(cx).log_file().is_some();
        let mut headline = summary::headline(&connection, status.as_ref(), &rates, &activity);
        if !rates.tracking() && self.confirm_stop {
            self.confirm_stop = false;
        }
        // Offer to start VRFT once it's known not to be running, and keep
        // the offer up while it starts or if starting failed.
        let offer_start = !online
            && (matches!(connection, Connection::Offline { .. }) || launch != LaunchState::Idle);
        // Why starting failed, across the card with its details to copy.
        let start_failure = match &launch {
            LaunchState::Failed(message) if offer_start => {
                headline.detail.clear();
                Some(Notice::new(Tone::Problem, message.clone()).details(message.clone()))
            }
            _ => None,
        };
        let module_failed =
            headline.tone == Tone::Problem && headline.fix == Some(Fix::Open(PageId::MODULES));
        // A module's own error is technical; it goes behind Details.
        let technical = module_failed.then(|| std::mem::take(&mut headline.detail));
        if module_failed {
            headline.detail = t!("home.module_failed").into();
        }
        let module_loaded = status
            .as_ref()
            .and_then(|status| status.daemon.as_ref())
            .and_then(|daemon| daemon.module.as_ref())
            .is_some_and(|module| module.loaded);
        let first_run = online && module_loaded && !rates.tracking() && !self.ever_live;
        // Setup's steps done so far: a module that loads, and somewhere to
        // send tracking. Putting the headset on is the last.
        let steps_done = usize::from(module_loaded)
            + usize::from(
                status
                    .as_ref()
                    .and_then(|status| status.daemon.as_ref())
                    .is_some_and(|daemon| daemon.output.is_some()),
            );

        // The band's buttons: Start while VRFT isn't running, a problem's
        // fix beside Stop, or Restart and Stop.
        let controls: Option<AnyElement> = if offer_start {
            Some(
                StartVrft::new(self.launcher.clone())
                    .buttons_only()
                    .into_any_element(),
            )
        } else if let Some(fix) = headline.fix {
            let button = match fix {
                Fix::Open(page) if page == PageId::MODULES => Button::new("fix-headline")
                    .label(t!("home.choose_module"))
                    .on_click(|_, _, cx| open_page(PageId::MODULES, cx)),
                fix => fix_button("fix-headline", fix),
            };
            Some(
                h_flex()
                    .gap_2()
                    .child(button.primary().regular())
                    .when(log, |row| {
                        row.child(
                            Button::new("open-log")
                                .ghost()
                                .regular()
                                .icon(IconName::ScrollText)
                                .label(t!("home.open_log"))
                                .on_click(|_, _, cx| open_page(PageId::LOGS, cx)),
                        )
                    })
                    .children(self.engine_controls(false, cx))
                    .into_any_element(),
            )
        } else {
            self.engine_controls(true, cx)
        };
        let config_error = status
            .as_ref()
            .and_then(|status| status.daemon.as_ref())
            .and_then(|daemon| daemon.config_error.clone())
            .map(|error| {
                v_flex()
                    .gap_2()
                    .items_start()
                    .child(Notice::new(Tone::Problem, summary::config_error(&error)))
                    .child(fix_button("fix-config", Fix::Config).outline().regular())
            });

        let problem = headline.tone == Tone::Problem;
        // On or stopped, the band says it all; otherwise a line under it
        // says what's happening or what to do.
        let detail = (headline.tone != Tone::Off && !headline.detail.is_empty())
            .then(|| headline.detail.clone());
        let (band_bg, band_line) = match headline.tone {
            Tone::Good => (palette::good_bg(), palette::good_line()),
            Tone::Problem => (palette::signal_bg(), palette::signal_line()),
            Tone::Waiting | Tone::Off => (palette::rail(), palette::line_soft()),
        };
        let band = h_flex()
            .h(px(56.))
            .pl(px(12.))
            .pr(px(20.))
            .gap_4()
            .rounded_t(cx.theme().radius_lg - px(1.))
            .bg(band_bg)
            .border_b_1()
            .border_color(band_line)
            .children(controls)
            .child(
                h_flex()
                    .ml_auto()
                    .min_w_0()
                    .gap(px(10.))
                    .child(state_mark(headline.tone))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(14.))
                            .font_medium()
                            .text_color(match headline.tone {
                                Tone::Problem => palette::signal_text(),
                                Tone::Off => palette::text_2(),
                                Tone::Good | Tone::Waiting => palette::text(),
                            })
                            .child(headline.value.clone()),
                    ),
            );
        let setup = first_run.then(|| {
            v_flex()
                .gap_3()
                .child(
                    h_flex()
                        .gap_3()
                        .child(h_flex().gap_1().children((0..3).map(|step| {
                            div()
                                .w(px(22.))
                                .h(px(3.))
                                .rounded_full()
                                .bg(if step < steps_done {
                                    palette::text()
                                } else {
                                    palette::line_strong()
                                })
                        })))
                        .child(
                            div()
                                .font_family(MONO_FONT)
                                .text_size(px(11.))
                                .text_color(palette::text_3())
                                .child(t!("home.steps_done", done = steps_done, total = 3)),
                        ),
                )
                .child(
                    div()
                        .text_size(px(26.))
                        .line_height(px(32.))
                        .font_semibold()
                        .child(t!("home.getting_started")),
                )
                .child(self.checklist(status.as_ref()))
        });
        let body = v_flex()
            .px(px(HERO_PADDING))
            .pt(px(22.))
            .pb(px(28.))
            .gap(px(18.))
            .children(detail.map(|detail| {
                div()
                    .text_size(px(13.5))
                    .text_color(palette::text_2())
                    .child(detail)
            }))
            .children(start_failure)
            .children(
                technical
                    .filter(|detail| !detail.is_empty())
                    .map(|detail| h_flex().child(self.details_row(detail, cx))),
            )
            .children(setup.map(|setup| {
                v_flex().gap(px(22.)).child(setup).child(
                    div()
                        .mx(px(-HERO_PADDING))
                        .h(px(1.))
                        .bg(palette::line_soft()),
                )
            }))
            .child(self.signal_path(status.as_ref(), &rates, cx));

        let available = vrft_gui_core::content_width(window);
        let sections = self.extension_sections(available, cx);
        let hero = card(cx)
            .when(problem, |card| card.border_color(palette::signal_line()))
            .flex()
            .flex_col()
            .child(band)
            .child(body);

        v_flex()
            .gap_6()
            .child(hero)
            .children(config_error)
            .children(sections)
    }
}

/// Where tracking goes, for the checklist, and whether that step is done.
/// For VRChat that's what OSCQuery has found; for other outputs, or a daemon
/// that doesn't report it, the address.
fn where_tracking_goes(output: &OutputTarget) -> (String, bool) {
    let local = matches!(output.address.as_str(), "127.0.0.1" | "localhost");
    let Some(vrchat) = output.vrchat.as_ref() else {
        let (label, address, port) = (output_label(&output.mode), &output.address, output.port);
        let text = if local {
            t!(
                "home.output_on_this_pc",
                output = label,
                address = address,
                port = port
            )
        } else {
            t!(
                "home.output_at",
                output = label,
                address = address,
                port = port
            )
        };
        return (text.into(), true);
    };
    let text = match (vrchat.found, vrchat.avatar_face_tracking) {
        (false, _) => t!("home.vrchat_waiting"),
        (true, None) => t!("home.vrchat_found"),
        (true, Some(true)) => t!("home.vrchat_face_tracking"),
        (true, Some(false)) => t!("home.vrchat_no_face_tracking"),
    };
    let text = if local {
        text.to_string()
    } else {
        t!("home.at_address", text = text, address = output.address).to_string()
    };
    let done = vrchat.found && vrchat.avatar_face_tracking != Some(false);
    (text, done)
}

fn output_label(mode: &str) -> String {
    match mode {
        "Generic" => t!("home.generic_udp").into(),
        mode => mode.into(),
    }
}

/// Which of Restart, Stop and End the status band shows.
#[derive(Debug, PartialEq)]
struct EngineButtons {
    restart: bool,
    stop: bool,
    end: bool,
}

/// Restart and Stop while VRFT runs, Restart only when asked for and not
/// while a stop waits to be confirmed. End once stopping has failed: beside
/// Stop, to try again, while VRFT still answers, and alone once it doesn't.
fn engine_buttons(
    running: bool,
    stuck: bool,
    with_restart: bool,
    confirm_stop: bool,
) -> EngineButtons {
    EngineButtons {
        restart: running && with_restart && !confirm_stop,
        stop: running,
        end: stuck,
    }
}

/// How things stand, at the status band's end: a green dot while tracking
/// is on, a hollow ring while it's stopped, and the usual marks otherwise.
fn state_mark(tone: Tone) -> AnyElement {
    match tone {
        Tone::Good => div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(18.))
            .rounded_full()
            .bg(palette::good().opacity(0.16))
            .child(div().size(px(10.)).rounded_full().bg(palette::good()))
            .into_any_element(),
        Tone::Off => div()
            .flex_none()
            .size(px(10.))
            .rounded_full()
            .border_2()
            .border_color(palette::line_focus())
            .into_any_element(),
        tone => StatusDot::new(tone).into_any_element(),
    }
}

/// A checklist step's mark: a white disc with a tick when done, a ring
/// around an arc while it's the one to do.
fn step_mark(done: bool, current: bool) -> impl IntoElement {
    let mark = div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(22.))
        .rounded_full();
    if done {
        mark.bg(palette::text())
            .text_color(palette::page())
            .child(Icon::new(IconName::Check).size(px(13.)))
    } else {
        mark.border_1()
            .border_color(if current {
                palette::text()
            } else {
                palette::line_focus()
            })
            .when(current, |mark| mark.child(StatusDot::new(Tone::Waiting)))
    }
}

fn checklist_row(
    label: impl Into<SharedString>,
    value: String,
    done: bool,
    change: Option<(&'static str, PageId)>,
) -> impl IntoElement {
    h_flex()
        .h(px(52.))
        .gap(px(14.))
        .border_b_1()
        .border_color(palette::line_soft())
        .child(step_mark(done, !done))
        .child(
            div()
                .w(px(200.))
                .flex_none()
                .text_sm()
                .font_medium()
                .child(label.into()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(13.))
                .text_color(palette::text_3())
                .child(value),
        )
        .children(change.map(|(id, page)| {
            Button::new(id)
                .ghost()
                .small()
                .label(t!("home.change"))
                .on_click(move |_, _, cx| open_page(page, cx))
        }))
}

/// One stage of the signal path: its name, a rate or value, and a mark for
/// how it stands. It opens the page that manages it.
fn path_node(
    id: (&'static str, usize),
    icon: IconName,
    label: String,
    value: Option<String>,
    tone: Tone,
    page: PageId,
) -> impl IntoElement {
    let problem = tone == Tone::Problem;
    let idle = matches!(tone, Tone::Off);
    let id = SharedString::from(format!("path-{}-{}", id.0, id.1));
    h_flex()
        .id(id)
        .h(px(NODE_HEIGHT))
        .gap_3()
        .pl(px(13.))
        .pr_4()
        .rounded(px(11.))
        .border_1()
        .border_color(if problem {
            palette::signal_line()
        } else {
            gpui_kit::rgb(0x26262b).into()
        })
        .bg(if problem {
            palette::signal_bg()
        } else {
            palette::inset()
        })
        .cursor_pointer()
        .hover(|style| style.border_color(palette::line_focus()))
        .on_click(move |_, _, cx| open_page(page, cx))
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(30.))
                .rounded(px(8.))
                .bg(if problem {
                    gpui_kit::rgb(0x2a1a0e).into()
                } else if idle {
                    gpui_kit::rgb(0x1a1a1d).into()
                } else {
                    Hsla::from(gpui_kit::rgb(0x222226))
                })
                .text_color(if problem {
                    palette::signal()
                } else if idle {
                    palette::text_3()
                } else {
                    gpui_kit::rgb(0xd4d4d4).into()
                })
                .child(Icon::new(icon).size(px(15.))),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(13.5))
                .font_medium()
                .text_color(if idle {
                    palette::text_2()
                } else {
                    palette::text()
                })
                .child(label),
        )
        .children(value.map(|value| {
            div()
                .flex_none()
                .font_family(MONO_FONT)
                .text_size(px(12.))
                .text_color(palette::text_2())
                .child(value)
        }))
        .child(StatusDot::new(tone))
}

/// The curves from each source into smoothing: solid white where data
/// flows, dashed grey where it doesn't.
fn join(flows: Vec<bool>, downstream: bool, height: f32) -> impl IntoElement {
    div().w(px(JOIN_WIDTH)).h(px(height)).flex_none().child(
        canvas(
            |_, _, _| {},
            move |bounds: Bounds<Pixels>, _, window, _| {
                let origin = bounds.origin;
                let at = |x: f32, y: f32| point(origin.x + px(x), origin.y + px(y));
                let middle = height / 2.;
                for (index, flowing) in flows.iter().enumerate() {
                    let y = NODE_HEIGHT / 2. + index as f32 * (NODE_HEIGHT + NODE_GAP);
                    let lit = *flowing && downstream;
                    let mut curve = if lit {
                        PathBuilder::stroke(px(1.5))
                    } else {
                        PathBuilder::stroke(px(1.5)).dash_array(&[px(3.), px(4.)])
                    };
                    curve.move_to(at(0., y));
                    curve.cubic_bezier_to(
                        at(JOIN_WIDTH, middle),
                        at(JOIN_WIDTH / 2., y),
                        at(JOIN_WIDTH / 2., middle),
                    );
                    if let Ok(path) = curve.build() {
                        window.paint_path(path, line_color(lit));
                    }
                }
            },
        )
        .size_full(),
    )
}

/// The line from smoothing to the output.
fn link(flowing: bool) -> impl IntoElement {
    div()
        .w(px(LINK_WIDTH))
        .h(px(NODE_HEIGHT))
        .flex_none()
        .child(
            canvas(
                |_, _, _| {},
                move |bounds: Bounds<Pixels>, _, window, _| {
                    let origin = bounds.origin;
                    let y = origin.y + px(NODE_HEIGHT / 2.);
                    let mut line = if flowing {
                        PathBuilder::stroke(px(1.5))
                    } else {
                        PathBuilder::stroke(px(1.5)).dash_array(&[px(3.), px(4.)])
                    };
                    line.move_to(point(origin.x, y));
                    line.line_to(point(origin.x + px(LINK_WIDTH), y));
                    if let Ok(path) = line.build() {
                        window.paint_path(path, line_color(flowing));
                    }
                },
            )
            .size_full(),
        )
}

fn line_color(flowing: bool) -> Hsla {
    if flowing {
        palette::text()
    } else {
        palette::line_strong()
    }
}

fn tile_grid(tiles: Vec<StatTile>, available: f32) -> AnyElement {
    let columns = fitting_columns(available).min(tiles.len().max(1) as u16);
    div()
        .grid()
        .grid_cols(columns)
        .gap(px(TILE_GAP))
        .children(tiles)
        .into_any_element()
}

fn fitting_columns(available: f32) -> u16 {
    (((available + TILE_GAP) / (MIN_TILE_WIDTH + TILE_GAP)).floor() as u16).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vrft_gui_core::client::VrchatLink;

    #[test]
    fn generic_output_reads_as_udp() {
        assert_eq!(output_label("Generic"), "Generic UDP");
        assert_eq!(output_label("VRChat"), "VRChat");
    }

    fn vrchat(address: &str, link: Option<VrchatLink>) -> OutputTarget {
        OutputTarget {
            mode: "VRChat".into(),
            address: address.into(),
            port: 9000,
            vrchat: link,
            ..OutputTarget::default()
        }
    }

    #[test]
    fn vrchat_reads_as_what_was_found_not_an_address() {
        let waiting = where_tracking_goes(&vrchat("127.0.0.1", Some(VrchatLink::default())));
        assert_eq!(waiting, (t!("home.vrchat_waiting").into(), false));

        let found = VrchatLink {
            found: true,
            avatar_face_tracking: Some(true),
            sending_to: "127.0.0.1:9000".into(),
        };
        let ready = where_tracking_goes(&vrchat("127.0.0.1", Some(found.clone())));
        assert_eq!(ready, (t!("home.vrchat_face_tracking").into(), true));

        let no_face = VrchatLink {
            avatar_face_tracking: Some(false),
            ..found
        };
        assert!(!where_tracking_goes(&vrchat("127.0.0.1", Some(no_face))).1);
    }

    #[test]
    fn vrchat_on_another_pc_names_its_address() {
        let (text, _) = where_tracking_goes(&vrchat("192.0.2.10", Some(VrchatLink::default())));
        assert!(text.contains("192.0.2.10"));
    }

    #[test]
    fn an_older_daemon_still_shows_the_address() {
        let (text, done) = where_tracking_goes(&vrchat("127.0.0.1", None));
        assert!(text.contains("127.0.0.1:9000"));
        assert!(done);
    }

    #[test]
    fn stop_comes_back_while_vrft_answers_after_stopping_failed() {
        let buttons = |restart, stop, end| EngineButtons { restart, stop, end };
        assert_eq!(
            engine_buttons(true, false, true, false),
            buttons(true, true, false)
        );
        assert_eq!(
            engine_buttons(true, true, true, false),
            buttons(true, true, true),
            "still answering, so Stop and Restart stay beside End"
        );
        assert_eq!(
            engine_buttons(false, true, true, false),
            buttons(false, false, true)
        );
        assert_eq!(
            engine_buttons(true, false, false, false),
            buttons(false, true, false),
            "beside a problem's fix"
        );
        assert_eq!(
            engine_buttons(true, false, true, true),
            buttons(false, true, false)
        );
        assert_eq!(
            engine_buttons(false, false, true, false),
            buttons(false, false, false)
        );
    }

    #[test]
    fn tiles_fill_the_width_they_have() {
        assert_eq!(fitting_columns(100.), 1);
        assert_eq!(fitting_columns(856.), 4);
    }
}
