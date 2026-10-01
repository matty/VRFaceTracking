//! Eyes: independent per-eye gaze from the Quest Pro, recentering, pupil
//! size from the eye cameras, and the settings that decide what VRFT sends
//! for each eye.
use crate::daemon::{PupilMark, Settings, SettingsPatch, Status, FRAME_WIDTH};
use crate::live::{CameraFeed, QuestProState};
use crate::pages;
use crate::speech::Speaker;
use crate::summary::Connection;
use crate::summary::{self, Tone};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex, Disableable as _, Icon, Sizable as _, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    black, canvas, div, fill, img, point, px, relative, rgb, size, AnyElement, App,
    AppContext as _, Bounds, Context, Div, Entity, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement, PathBuilder, Pixels, Render, RenderImage, RenderOnce, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled, StyledImage as _, Subscription, Task, Window,
};
use rust_i18n::t;
use std::sync::Arc;
use std::time::Duration;
use vrft_gui_core::extension::open_page;
use vrft_gui_core::launcher::{Launcher, StartVrft};
use vrft_gui_core::palette::{self, MONO_FONT};
use vrft_gui_core::widgets::{
    card, fix_button, mono, ButtonExt as _, EmptyState, Meter, Notice, PageHeader, StatusLine,
    StatusPill,
};

/// Below this content width the columns stack.
const TWO_COLUMN_WIDTH: f32 = 760.;
/// The right-hand column: recentering and the output settings.
const SIDE_WIDTH: f32 = 320.;
/// Between the page's cards.
const GAP: f32 = 20.;
/// An eye snapshot older than this is not shown, as in the preview page.
const SNAPSHOT_STALE_MS: u64 = 3000;
const RECENTER_COUNTDOWN: u8 = 3;
/// The recenter target's size.
const TARGET_SIZE: f32 = 112.;

enum Recenter {
    Idle,
    Counting(u8),
    Saving,
}

/// The card an outcome is shown in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spot {
    Recenter,
    /// Tracking each eye separately.
    Tracking,
    /// Mirroring and swapping the eyes.
    Output,
    /// Measuring pupil size.
    Pupils,
}

pub struct EyesPage {
    daemon: Entity<QuestProState>,
    snapshots: Entity<CameraFeed>,
    launcher: Entity<Launcher>,
    recenter: Recenter,
    /// The outcome of the last recenter or settings change, and the card
    /// it's about.
    message: Option<(Spot, Notice)>,
    /// Reads the recenter countdown aloud, for someone in the headset.
    speaker: Speaker,
    /// The mirror and swap switches show.
    output_open: bool,
    /// The details show.
    details_open: bool,
    pupil_smoothing: Entity<SliderState>,
    /// The pupil smoothing the daemon last reported, so the slider only
    /// follows changes made elsewhere, not every status.
    shown_smoothing: Option<f32>,
    _action: Option<Task<()>>,
    _subscriptions: [Subscription; 4],
}

impl EyesPage {
    pub fn new(
        daemon: Entity<QuestProState>,
        snapshots: Entity<CameraFeed>,
        launcher: Entity<Launcher>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let pupil_smoothing = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(100.)
                .step(1.)
                .default_value(Settings::default().pupil_smoothing)
        });
        let subscriptions = [
            cx.observe_in(&daemon, window, |page, _, window, cx| {
                page.follow_smoothing(window, cx);
                cx.notify();
            }),
            cx.observe(&snapshots, |_, _, cx| cx.notify()),
            cx.observe(&launcher, |_, _, cx| cx.notify()),
            // The number follows the drag; the setting saves on release.
            cx.subscribe(
                &pupil_smoothing,
                |page, _, event: &SliderEvent, cx| match event {
                    SliderEvent::Change(_) => cx.notify(),
                    SliderEvent::Release(value) => page.save_smoothing(value.start().round(), cx),
                },
            ),
        ];
        Self {
            daemon,
            snapshots,
            launcher,
            recenter: Recenter::Idle,
            message: None,
            speaker: Speaker::default(),
            output_open: false,
            details_open: false,
            pupil_smoothing,
            shown_smoothing: None,
            _action: None,
            _subscriptions: subscriptions,
        }
    }

    /// Moves the slider to a pupil smoothing changed elsewhere.
    fn follow_smoothing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(value) = self
            .daemon
            .read(cx)
            .status()
            .map(|status| status.settings.pupil_smoothing)
        else {
            return;
        };
        if self.shown_smoothing != Some(value) {
            self.shown_smoothing = Some(value);
            self.pupil_smoothing
                .update(cx, |slider, cx| slider.set_value(value, window, cx));
        }
    }

    /// Shows the new pupil smoothing straight away, then saves it.
    fn save_smoothing(&mut self, value: f32, cx: &mut Context<Self>) {
        let Some(mut settings) = self
            .daemon
            .read(cx)
            .status()
            .map(|status| status.settings.clone())
        else {
            return;
        };
        settings.pupil_smoothing = value;
        self.shown_smoothing = Some(value);
        self.daemon
            .update(cx, |daemon, cx| daemon.show_settings(settings, cx));
        let client = self.daemon.read(cx).client();
        let patch = SettingsPatch {
            pupil_smoothing: Some(value),
            ..SettingsPatch::default()
        };
        self.run(
            move || client.update_settings(&patch),
            None,
            t!("eyes.couldnt_save").into(),
            Spot::Pupils,
            cx,
        );
    }

    /// Counts down so the wearer can look away from the screen, then saves
    /// the last second of gaze as straight ahead.
    fn start_recenter(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.recenter, Recenter::Idle) {
            return;
        }
        let client = self.daemon.read(cx).client();
        self.message = None;
        self._action = Some(cx.spawn(async move |this, cx| {
            for remaining in (1..=RECENTER_COUNTDOWN).rev() {
                let counted = this.update(cx, |page, cx| {
                    page.recenter = Recenter::Counting(remaining);
                    page.speaker.say(&if remaining == RECENTER_COUNTDOWN {
                        t!("eyes.recenter_first_count", count = remaining).into_owned()
                    } else {
                        remaining.to_string()
                    });
                    cx.notify();
                });
                if counted.is_err() {
                    return;
                }
                cx.background_executor().timer(Duration::from_secs(1)).await;
            }
            let saving = this.update(cx, |page, cx| {
                page.recenter = Recenter::Saving;
                cx.notify();
            });
            if saving.is_err() {
                return;
            }
            let result = cx
                .background_executor()
                .spawn(async move { client.recenter_eyes() })
                .await;
            this.update(cx, |page, cx| {
                page.recenter = Recenter::Idle;
                page.speaker.say(&if result.is_ok() {
                    t!("eyes.recentered")
                } else {
                    t!("eyes.recenter_failed")
                });
                page.finish(
                    result,
                    Some(t!("eyes.recentered_notice").into()),
                    t!("eyes.recenter_failed").into(),
                    Spot::Recenter,
                    cx,
                );
            })
            .ok();
        }));
    }

    fn undo_recenter(&mut self, cx: &mut Context<Self>) {
        let client = self.daemon.read(cx).client();
        self.run(
            move || client.clear_eye_recenter(),
            Some(t!("eyes.recenter_undone").into()),
            t!("eyes.couldnt_undo").into(),
            Spot::Recenter,
            cx,
        );
    }

    /// Saves one setting. The switch moves straight away and the daemon's
    /// answer then confirms or corrects it, so only a failure is reported.
    fn change(&mut self, key: &'static str, value: bool, cx: &mut Context<Self>) {
        let Some(mut settings) = self
            .daemon
            .read(cx)
            .status()
            .map(|status| status.settings.clone())
        else {
            return;
        };
        match key {
            "eye_gaze" => settings.eye_gaze = value,
            "eye_swap_output" => settings.eye_swap_output = value,
            "eye_invert_yaw" => settings.eye_invert_yaw = value,
            "pupils" => settings.pupils = value,
            "pupil_hold_closed" => settings.pupil_hold_closed = value,
            _ => return,
        }
        self.daemon
            .update(cx, |daemon, cx| daemon.show_settings(settings, cx));
        let client = self.daemon.read(cx).client();
        let patch = SettingsPatch {
            eye_gaze: (key == "eye_gaze").then_some(value),
            eye_swap_output: (key == "eye_swap_output").then_some(value),
            eye_invert_yaw: (key == "eye_invert_yaw").then_some(value),
            pupils: (key == "pupils").then_some(value),
            pupil_hold_closed: (key == "pupil_hold_closed").then_some(value),
            ..SettingsPatch::default()
        };
        let spot = match key {
            "eye_gaze" => Spot::Tracking,
            "pupils" | "pupil_hold_closed" => Spot::Pupils,
            _ => Spot::Output,
        };
        self.run(
            move || client.update_settings(&patch),
            None,
            t!("eyes.couldnt_save").into(),
            spot,
            cx,
        );
    }

    /// Puts the output switches back to how they ship.
    fn reset_output(&mut self, cx: &mut Context<Self>) {
        let defaults = Settings::default();
        let Some(mut settings) = self
            .daemon
            .read(cx)
            .status()
            .map(|status| status.settings.clone())
        else {
            return;
        };
        settings.eye_swap_output = defaults.eye_swap_output;
        settings.eye_invert_yaw = defaults.eye_invert_yaw;
        self.daemon
            .update(cx, |daemon, cx| daemon.show_settings(settings, cx));
        let client = self.daemon.read(cx).client();
        let patch = SettingsPatch {
            eye_swap_output: Some(defaults.eye_swap_output),
            eye_invert_yaw: Some(defaults.eye_invert_yaw),
            ..SettingsPatch::default()
        };
        self.run(
            move || client.update_settings(&patch),
            Some(t!("eyes.output_reset").into()),
            t!("eyes.couldnt_save").into(),
            Spot::Output,
            cx,
        );
    }

    fn run(
        &mut self,
        request: impl FnOnce() -> anyhow::Result<Settings> + Send + 'static,
        success: Option<SharedString>,
        failure: SharedString,
        spot: Spot,
        cx: &mut Context<Self>,
    ) {
        self.message = None;
        self._action = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { request() })
                .await;
            this.update(cx, |page, cx| {
                page.finish(result, success, failure, spot, cx)
            })
            .ok();
        }));
    }

    fn finish(
        &mut self,
        result: anyhow::Result<Settings>,
        success: Option<SharedString>,
        failure: SharedString,
        spot: Spot,
        cx: &mut Context<Self>,
    ) {
        let notice = match result {
            Ok(settings) => {
                self.daemon
                    .update(cx, |daemon, cx| daemon.show_settings(settings, cx));
                success.map(|text| Notice::new(Tone::Good, text))
            }
            Err(error) => Some(Notice::error(&failure, &error)),
        };
        self.message = notice.map(|notice| (spot, notice));
        cx.notify();
    }

    fn message_at(&self, spot: Spot) -> Option<Notice> {
        self.message
            .as_ref()
            .filter(|(shown_at, _)| *shown_at == spot)
            .map(|(_, notice)| notice.clone())
    }

    /// Recentering, with a target that counts down, and what keeps it from
    /// working when something does.
    fn recenter_card(&self, status: Option<&Status>, cx: &mut Context<Self>) -> AnyElement {
        let settings = status.map(|status| &status.settings);
        let online = status.is_some();
        // Recentering averages the last second of gaze, so it needs gaze.
        let eye_data = status.is_some_and(|status| status.eyes.fresh);
        let recentered = settings.is_some_and(|settings| settings.eye_offsets.is_some());
        let reading = summary::eyes(status);
        let idle = matches!(self.recenter, Recenter::Idle);
        let label: SharedString = match self.recenter {
            Recenter::Idle => t!("eyes.recenter").into(),
            Recenter::Counting(_) => t!("eyes.look_ahead").into(),
            Recenter::Saving => t!("eyes.recentering").into(),
        };
        // A big count, so it can be read from the corner of an eye.
        let centre: AnyElement = match self.recenter {
            Recenter::Counting(remaining) => div()
                .font_family(MONO_FONT)
                .text_size(px(44.))
                .line_height(px(48.))
                .font_semibold()
                .text_color(palette::text())
                .child(remaining.to_string())
                .into_any_element(),
            _ => div()
                .text_color(palette::text())
                .child(Icon::new(IconName::Crosshair).size(px(34.)))
                .into_any_element(),
        };
        // What's wrong, and the way to the fix when it's on another page.
        let problem =
            (reading.tone == Tone::Problem || reading.fix.is_some()) && !reading.detail.is_empty();
        let fix = reading.fix.map(|fix| fix_button("fix-eyes", fix).regular());
        let connection = self.daemon.read(cx).connection().clone();
        let offline = matches!(connection, Connection::Offline { .. });
        let not_running =
            connection == Connection::Online && self.daemon.read(cx).status().is_none();
        let launcher = self.launcher.clone();
        card(cx)
            .px(px(18.))
            .pt(px(20.))
            .pb_4()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(14.))
            .child(recenter_target(centre))
            .child(
                v_flex()
                    .gap(px(5.))
                    .items_center()
                    .text_center()
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_semibold()
                            .text_color(palette::text())
                            .child(t!("eyes.recenter")),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .line_height(px(18.))
                            .text_color(palette::text_2())
                            .child(t!("eyes.recenter_hint")),
                    ),
            )
            // Per-eye and its rate are in the page's header.
            .when(
                online && !problem && !reading.detail.is_empty() && reading.tone != Tone::Good,
                |card| {
                    card.child(
                        div()
                            .w_full()
                            .child(StatusLine::new(reading.tone, reading.detail.clone())),
                    )
                },
            )
            .when(problem, |card| {
                card.child(
                    v_flex()
                        .w_full()
                        .gap_2()
                        .items_start()
                        .child(Notice::new(reading.tone, reading.detail.clone()))
                        .children(fix),
                )
            })
            .when(offline, |card| {
                card.child(div().w_full().child(StartVrft::new(self.launcher.clone())))
            })
            .when(not_running, |card| {
                card.child(
                    v_flex()
                        .w_full()
                        .gap_2()
                        .items_start()
                        .child(Notice::new(Tone::Problem, t!("eyes.support_not_running")))
                        .child(
                            Button::new("restart-vrft")
                                .regular()
                                .icon(IconName::RotateCcw)
                                .label(t!("eyes.restart_vrft"))
                                .on_click(move |_, _, cx| {
                                    launcher.update(cx, |launcher, cx| launcher.restart(cx))
                                }),
                        ),
                )
            })
            .child(
                Button::new("recenter")
                    .primary()
                    .prominent()
                    .w_full()
                    .label(label)
                    .loading(matches!(self.recenter, Recenter::Saving))
                    .disabled(!online || !eye_data || !idle)
                    .on_click(cx.listener(|page, _, _, cx| page.start_recenter(cx))),
            )
            // The reading above says why, when it has something to say.
            .when(online && !eye_data && reading.detail.is_empty(), |card| {
                card.child(
                    div()
                        .w_full()
                        .text_center()
                        .text_xs()
                        .text_color(palette::text_3())
                        .child(t!("eyes.waiting_for_eye_data")),
                )
            })
            .when(recentered, |card| {
                card.child(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .child(
                            h_flex()
                                .gap(px(7.))
                                .text_xs()
                                .text_color(palette::text_2())
                                .child(Icon::new(IconName::Check).size(px(13.)))
                                .child(t!("eyes.recentered")),
                        )
                        .child(
                            div().mr(px(-8.)).child(
                                Button::new("undo-recenter")
                                    .ghost()
                                    .small()
                                    .label(t!("eyes.undo"))
                                    .tooltip(t!("eyes.undo_recenter"))
                                    .disabled(!idle)
                                    .on_click(cx.listener(|page, _, _, cx| page.undo_recenter(cx))),
                            ),
                        ),
                )
            })
            .children(
                self.message_at(Spot::Recenter)
                    .map(|notice| div().w_full().child(notice)),
            )
            .into_any_element()
    }

    /// Whether VRFT sends each eye its own gaze.
    fn tracking_card(&self, status: Option<&Status>, cx: &mut Context<Self>) -> AnyElement {
        let online = status.is_some();
        let checked = status.map_or(Settings::default().eye_gaze, |status| {
            status.settings.eye_gaze
        });
        card(cx)
            .px(px(18.))
            .py_4()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                h_flex()
                    .items_start()
                    .gap(px(14.))
                    .child(setting_text(
                        t!("eyes.track_each_eye"),
                        t!("eyes.track_each_eye_hint"),
                    ))
                    .child(
                        div().flex_none().mt(px(1.)).child(
                            Switch::new("eye-gaze")
                                .accessibility_label(t!("eyes.track_each_eye"))
                                .checked(checked)
                                .disabled(!online)
                                .on_change(cx.listener(|page, checked: &bool, _, cx| {
                                    page.change("eye_gaze", *checked, cx)
                                })),
                        ),
                    ),
            )
            .children(self.message_at(Spot::Tracking))
            .into_any_element()
    }

    /// Whether VRFT measures pupil size, and what it measures.
    fn pupils_card(&self, status: Option<&Status>, cx: &mut Context<Self>) -> AnyElement {
        let online = status.is_some();
        let checked = status.map_or(Settings::default().pupils, |status| status.settings.pupils);
        let pupils = status.map(|status| &status.eyes.pupils);
        let snapshots = status
            .and_then(|status| status.eye_frame_age_ms)
            .is_some_and(|age| age <= SNAPSHOT_STALE_MS);
        let millimetres = |value: Option<f32>| {
            value.map_or_else(
                || "\u{2014}".to_string(),
                |mm| t!("eyes.millimetres", mm = format!("{mm:.1}")).into_owned(),
            )
        };
        let readings = pupils
            .filter(|pupils| checked && pupils.fresh)
            .map(|pupils| {
                let dilation = pupils.dilation.unwrap_or(0.);
                v_flex()
                    .child(reading_row(
                        t!("eyes.dilation"),
                        h_flex()
                            .justify_end()
                            .gap(px(10.))
                            .child(
                                div()
                                    .w(px(80.))
                                    .child(Meter::new(dilation, palette::text())),
                            )
                            .child(mono(format!("{:.0}%", dilation * 100.)).w(px(34.))),
                    ))
                    .child(reading_row(
                        t!("eyes.left_pupil"),
                        mono(millimetres(pupils.diameter_mm[0])),
                    ))
                    .child(reading_row(
                        t!("eyes.right_pupil"),
                        mono(millimetres(pupils.diameter_mm[1])),
                    ))
            });
        let smoothing = self.pupil_smoothing.read(cx).value().start();
        let smoothing_row = h_flex()
            .gap(px(14.))
            .child(
                h_flex()
                    .id("pupil-smoothing-label")
                    .flex_none()
                    .gap_1p5()
                    .text_size(px(13.))
                    .text_color(palette::text_2())
                    .child(t!("eyes.pupil_smoothing"))
                    .child(
                        div()
                            .text_color(palette::text_4())
                            .child(Icon::new(IconName::Info).size(px(13.))),
                    )
                    .tooltip(|window, cx| {
                        Tooltip::new(SharedString::from(t!("eyes.pupil_smoothing_note")))
                            .build(window, cx)
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Slider::new(&self.pupil_smoothing).disabled(!online || !checked)),
            )
            .child(
                mono(format!("{smoothing:.0}"))
                    .w(px(32.))
                    .flex_none()
                    .text_right(),
            );
        let hold = status.map_or(Settings::default().pupil_hold_closed, |status| {
            status.settings.pupil_hold_closed
        });
        let hold_row = h_flex()
            .items_start()
            .gap(px(14.))
            .child(setting_text(
                t!("eyes.pupil_hold_closed"),
                t!("eyes.pupil_hold_closed_hint"),
            ))
            .child(
                div().flex_none().mt(px(1.)).child(
                    Switch::new("pupil-hold-closed")
                        .accessibility_label(t!("eyes.pupil_hold_closed"))
                        .checked(hold)
                        .disabled(!online || !checked)
                        .on_change(cx.listener(|page, checked: &bool, _, cx| {
                            page.change("pupil_hold_closed", *checked, cx)
                        })),
                ),
            );
        // Why the size isn't changing, while the eyes are closed.
        let held = pupils
            .filter(|pupils| checked && pupils.fresh && pupils.closed)
            .map(|_| {
                div()
                    .text_xs()
                    .text_color(palette::text_3())
                    .child(t!("eyes.pupils_held_closed"))
            });
        // How the snapshot shows what is found, while snapshots come.
        let outlined = (online && checked && snapshots).then(|| {
            div()
                .text_xs()
                .text_color(palette::text_3())
                .child(t!("eyes.pupils_outlined"))
        });
        // Why nothing is measured yet, while it's on.
        let waiting = (online && checked && readings.is_none()).then(|| {
            div()
                .text_xs()
                .text_color(palette::text_3())
                .child(if snapshots {
                    t!("eyes.pupils_looking")
                } else {
                    t!("eyes.pupils_need_snapshots")
                })
        });
        card(cx)
            .px(px(18.))
            .py_4()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                h_flex()
                    .items_start()
                    .gap(px(14.))
                    .child(setting_text(t!("eyes.pupils"), t!("eyes.pupils_hint")))
                    .child(
                        div().flex_none().mt(px(1.)).child(
                            Switch::new("pupils")
                                .accessibility_label(t!("eyes.pupils"))
                                .checked(checked)
                                .disabled(!online)
                                .on_change(cx.listener(|page, checked: &bool, _, cx| {
                                    page.change("pupils", *checked, cx)
                                })),
                        ),
                    ),
            )
            .child(smoothing_row)
            .child(hold_row)
            .children(readings)
            .children(held)
            .children(waiting)
            .children(outlined)
            .children(self.message_at(Spot::Pupils))
            .into_any_element()
    }

    /// The fixes for an avatar whose eyes look wrong, and the details, each
    /// behind a row that opens it.
    fn more_card(&self, status: Option<&Status>, cx: &mut Context<Self>) -> AnyElement {
        let settings = status.map(|status| &status.settings);
        let online = status.is_some();
        let defaults = Settings::default();
        let output = v_flex()
            .gap_4()
            .px_4()
            .pb(px(14.))
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_3())
                    .child(t!("eyes.output_hint")),
            )
            .child(
                h_flex()
                    .items_start()
                    .gap_3()
                    .child(setting_text(t!("eyes.mirror"), t!("eyes.mirror_hint")))
                    .child(
                        Switch::new("eye-invert")
                            .accessibility_label(t!("eyes.mirror"))
                            .checked(settings.map_or(defaults.eye_invert_yaw, |settings| {
                                settings.eye_invert_yaw
                            }))
                            .disabled(!online)
                            .on_change(cx.listener(|page, checked: &bool, _, cx| {
                                page.change("eye_invert_yaw", *checked, cx)
                            })),
                    ),
            )
            .child(
                h_flex()
                    .items_start()
                    .gap_3()
                    .child(setting_text(t!("eyes.swap"), t!("eyes.swap_hint")))
                    .child(
                        Switch::new("eye-swap")
                            .accessibility_label(t!("eyes.swap"))
                            .checked(settings.map_or(defaults.eye_swap_output, |settings| {
                                settings.eye_swap_output
                            }))
                            .disabled(!online)
                            .on_change(cx.listener(|page, checked: &bool, _, cx| {
                                page.change("eye_swap_output", *checked, cx)
                            })),
                    ),
            )
            .child(
                div().ml(px(-8.)).child(
                    Button::new("reset-output")
                        .ghost()
                        .small()
                        .label(t!("eyes.reset_to_defaults"))
                        .disabled(!online)
                        .on_click(cx.listener(|page, _, _, cx| page.reset_output(cx))),
                ),
            )
            .children(self.message_at(Spot::Output));

        let sample = status.and_then(|status| status.eyes.sample.as_ref());
        let eyes = status.map(|status| &status.eyes);
        let details = v_flex()
            .px_4()
            .pb_2()
            .child(reading_row(
                t!("eyes.engine_profile"),
                div().child(
                    sample
                        .map(|sample| sample.engine_profile.to_string())
                        .unwrap_or_else(|| "\u{2014}".into()),
                ),
            ))
            .child(reading_row(
                t!("eyes.per_eye_model"),
                div().child(match sample {
                    Some(sample) if sample.model_patched => t!("eyes.active"),
                    Some(_) => t!("eyes.not_active"),
                    None => "\u{2014}".into(),
                }),
            ))
            .child(reading_row(
                t!("eyes.recentered"),
                div().child(match status {
                    Some(status) if status.settings.eye_offsets.is_some() => t!("eyes.yes"),
                    Some(_) => t!("eyes.no"),
                    None => "\u{2014}".into(),
                }),
            ))
            .child(reading_row(
                t!("eyes.calibration_file"),
                mono(
                    eyes.and_then(|eyes| {
                        eyes.calibration
                            .rsplit(['\\', '/'])
                            .next()
                            .filter(|name| !name.is_empty())
                            .map(str::to_string)
                    })
                    .unwrap_or_else(|| "\u{2014}".into()),
                ),
            ))
            .child(reading_row(
                t!("eyes.meeting_distance"),
                div().child(match eyes {
                    Some(eyes) if eyes.convergence_calibrated => t!("eyes.calibrated"),
                    Some(eyes) if !eyes.calibration.is_empty() => t!("eyes.demo_calibration"),
                    _ => "\u{2014}".into(),
                }),
            ));

        card(cx)
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                disclosure(
                    "eyes-output",
                    t!("eyes.eyes_look_wrong"),
                    Some(t!("eyes.eyes_look_wrong_sub").into()),
                    self.output_open,
                )
                .on_click(cx.listener(|page, _, _, cx| {
                    page.output_open = !page.output_open;
                    cx.notify();
                })),
            )
            .when(self.output_open, |card| card.child(output))
            .child(
                disclosure("eyes-details", t!("eyes.details"), None, self.details_open)
                    .border_t_1()
                    .border_color(palette::line_soft())
                    .on_click(cx.listener(|page, _, _, cx| {
                        page.details_open = !page.details_open;
                        cx.notify();
                    })),
            )
            .when(self.details_open, |card| card.child(details))
            .into_any_element()
    }
}

impl Render for EyesPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.daemon.read(cx);
        let status = state.status().cloned();
        let status = status.as_ref();
        let reading = summary::eyes(status);
        let output = status.and_then(|status| status.eye_output_deg);
        let snapshot = status.and_then(|status| {
            status
                .eye_frame_age_ms
                .filter(|age| *age <= SNAPSHOT_STALE_MS)
                .map(|age| (status.eye_frame_sequence.unwrap_or_default(), age))
        });
        let image = snapshot.and(self.snapshots.read(cx).image());
        let pupils = snapshot
            .and(self.snapshots.read(cx).pupils())
            .filter(|_| status.is_some_and(|status| status.settings.pupils));
        let two_columns = vrft_gui_core::content_width(window) >= TWO_COLUMN_WIDTH;
        // "Per-eye · 90 Hz" while it works; otherwise the state alone.
        let pill = if reading.tone == Tone::Good && !reading.detail.is_empty() {
            t!("eyes.pill", value = reading.value, detail = reading.detail).into_owned()
        } else {
            reading.value.clone()
        };

        let gaze_rows: Vec<Div> = match output {
            Some([left, right]) => vec![
                reading_row(t!("eyes.avatar_left_eye"), mono(eye_angles(left))),
                reading_row(t!("eyes.avatar_right_eye"), mono(eye_angles(right))),
                reading_row(
                    t!("eyes.eyes_meet"),
                    div().child(summary::eyes_meet(left[0], right[0])),
                ),
            ],
            None => vec![reading_row(
                t!("eyes.gaze"),
                div().child(t!("eyes.combined")),
            )],
        };
        // Per-eye gaze through the headset's stock model moves both eyes as one.
        let stock_model = output.is_some()
            && status
                .and_then(|status| status.eyes.sample.as_ref())
                .is_some_and(|sample| !sample.model_patched);
        let gaze = card(cx)
            .px(px(18.))
            .pt_4()
            .pb(px(6.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                h_flex()
                    .justify_between()
                    .flex_wrap()
                    .gap_x_4()
                    .gap_y_1()
                    .child(card_title(t!("eyes.gaze_from_above")))
                    .child(legend()),
            )
            .when(stock_model, |card| {
                card.child(
                    v_flex()
                        .gap_2()
                        .items_start()
                        .child(Notice::new(Tone::Waiting, t!("eyes.stock_model")))
                        .child(
                            Button::new("open-headset-eyes")
                                .small()
                                .icon(IconName::Glasses)
                                .label(t!("eyes.open_headset"))
                                .on_click(|_, _, cx| open_page(pages::HEADSET, cx)),
                        ),
                )
            })
            .child(GazeView { output })
            .child(v_flex().children(gaze_rows));

        // Not under "No eye camera images" before the first one downloads.
        let snapshot = snapshot.filter(|_| image.is_some());
        let cameras = card(cx)
            .px(px(18.))
            .py(px(14.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                h_flex()
                    .justify_between()
                    .gap_4()
                    .child(card_title(t!("eyes.eye_cameras")))
                    .children(snapshot.map(|(sequence, age)| {
                        mono(t!("eyes.snapshot_age", sequence = sequence, age = age))
                            .text_size(px(11.5))
                            .text_color(palette::text_3())
                    })),
            )
            .child(SnapshotView { image, pupils });

        let left = v_flex().gap_4().child(gaze).child(cameras);
        let right = v_flex()
            .gap_4()
            .child(self.recenter_card(status, cx))
            .child(self.tracking_card(status, cx))
            .child(self.pupils_card(status, cx))
            .child(self.more_card(status, cx));
        let columns = if two_columns {
            h_flex()
                .items_start()
                .gap(px(GAP))
                .child(left.flex_1().min_w_0())
                .child(right.w(px(SIDE_WIDTH)).flex_none())
        } else {
            v_flex().gap_4().child(left).child(right)
        };

        v_flex()
            .gap(px(GAP))
            .child(
                PageHeader::new(t!("eyes.title"))
                    .description(t!("eyes.description"))
                    .trailing(StatusPill::new(reading.tone, pill)),
            )
            .child(columns)
    }
}

/// One eye's gaze in words: "8.7° right · vertical +1.2°".
fn eye_angles([yaw, pitch]: [f32; 2]) -> String {
    t!(
        "eyes.eye_angles",
        across = summary::across(yaw),
        vertical = summary::signed_degrees(pitch)
    )
    .into_owned()
}

fn card_title(title: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(14.))
        .font_semibold()
        .text_color(palette::text())
        .child(title.into())
}

/// A setting's name and what it does, beside its switch.
fn setting_text(label: impl Into<SharedString>, hint: impl Into<SharedString>) -> Div {
    v_flex()
        .flex_1()
        .min_w_0()
        .gap_1()
        .child(
            div()
                .text_size(px(13.5))
                .font_medium()
                .text_color(palette::text())
                .child(label.into()),
        )
        .child(
            div()
                .text_xs()
                .line_height(px(17.))
                .text_color(palette::text_3())
                .child(hint.into()),
        )
}

/// A label and its value on a line of a card, under a rule.
fn reading_row(label: impl Into<SharedString>, value: Div) -> Div {
    h_flex()
        .min_h(px(32.))
        .py(px(6.))
        .gap_3()
        .border_t_1()
        .border_color(palette::line_soft())
        .text_size(px(12.5))
        .child(
            div()
                .flex_none()
                .text_color(palette::text_3())
                .child(label.into()),
        )
        .child(
            value
                .flex_1()
                .min_w_0()
                .text_right()
                .text_color(palette::text()),
        )
}

/// A full-width row that shows or hides what's under it, with an optional
/// quieter line under its label and a chevron saying which.
fn disclosure(
    id: &'static str,
    label: impl Into<SharedString>,
    sub: Option<SharedString>,
    open: bool,
) -> Stateful<Div> {
    h_flex()
        .id(id)
        .gap_2p5()
        .px_4()
        .py(px(13.))
        .cursor_pointer()
        .hover(|style| style.bg(palette::inset()))
        .text_size(px(13.))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(div().text_color(rgb(0xd4d4d4)).child(label.into()))
                .children(sub.map(|sub| div().text_xs().text_color(palette::text_3()).child(sub))),
        )
        .child(
            div().flex_none().text_color(palette::text_3()).child(
                Icon::new(if open {
                    IconName::ChevronUp
                } else {
                    IconName::ChevronDown
                })
                .size(px(14.)),
            ),
        )
}

/// What the gaze drawing's marks mean: the left eye's ray solid, the
/// right's dashed, and a ring where they meet.
fn legend() -> impl IntoElement {
    let entry = |mark: AnyElement, label: SharedString| h_flex().gap_1p5().child(mark).child(label);
    let solid = div()
        .w(px(18.))
        .h(px(1.5))
        .bg(palette::text())
        .into_any_element();
    let dashed = h_flex()
        .w(px(18.))
        .gap(px(3.))
        .children((0..3).map(|_| div().w(px(3.)).h(px(1.5)).bg(palette::text())))
        .into_any_element();
    let ring = div()
        .size(px(9.))
        .rounded_full()
        .border_1()
        .border_color(palette::text())
        .into_any_element();
    h_flex()
        .gap(px(14.))
        .text_size(px(11.5))
        .text_color(palette::text_3())
        .child(entry(solid, t!("eyes.left_eye").into()))
        .child(entry(dashed, t!("eyes.right_eye").into()))
        .child(entry(ring, t!("eyes.where_they_meet").into()))
}

/// A thin ring with a dashed ring inside it, around what's in its centre:
/// a crosshair, or the countdown while recentering.
fn recenter_target(centre: AnyElement) -> impl IntoElement {
    let inset = 14.;
    div()
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(TARGET_SIZE))
        .rounded_full()
        .border_1()
        .border_color(palette::line_strong())
        .bg(palette::sunken())
        .child(
            div()
                .absolute()
                .top(px(inset))
                .left(px(inset))
                .size(px(TARGET_SIZE - 2. * inset - 2.))
                .rounded_full()
                .border_1()
                .border_dashed()
                .border_color(palette::line_strong()),
        )
        .child(centre)
}

/// Both gaze rays drawn from above, to scale with a typical 63 mm eye
/// separation: the face at the bottom, looking up the view.
#[derive(IntoElement)]
struct GazeView {
    output: Option<[[f32; 2]; 2]>,
}

/// The drawing's reference width; distances scale with the actual width.
const GAZE_REFERENCE_WIDTH: f32 = 480.;
const GAZE_PIXELS_PER_METRE: f32 = 300.;
const HALF_EYE_SEPARATION_M: f32 = 0.0315;
const GAZE_DISTANCES_M: [f32; 2] = [0.25, 0.5];

impl RenderOnce for GazeView {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let output = self.output;
        // Distance labels sit at fixed fractions of the height, because the
        // drawing keeps a 2:1 shape at any width.
        let labels = GAZE_DISTANCES_M.map(|metres| {
            let unit_height = GAZE_REFERENCE_WIDTH / 2.;
            let y = (unit_height - 16. - metres * GAZE_PIXELS_PER_METRE) / unit_height;
            div()
                .absolute()
                .left(px(10.))
                .top(relative(y))
                .mt(px(-17.))
                .font_family(MONO_FONT)
                .text_size(px(10.))
                .text_color(palette::text_3())
                .child(t!("eyes.metres", metres = metres))
        });
        div()
            .relative()
            .w_full()
            .aspect_ratio(2.)
            .rounded(px(10.))
            .overflow_hidden()
            .bg(palette::sunken())
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds: Bounds<Pixels>, _, window, _| {
                        let width = bounds.size.width.as_f32();
                        let unit = width / GAZE_REFERENCE_WIDTH;
                        let scale = GAZE_PIXELS_PER_METRE * unit;
                        let origin = bounds.origin;
                        let at = |x: f32, y: f32| point(origin.x + px(x), origin.y + px(y));
                        let base = bounds.size.height.as_f32() - 16. * unit;
                        let middle = width / 2.;
                        let guide = palette::line_strong();
                        let ink = palette::text();

                        for metres in GAZE_DISTANCES_M {
                            let y = base - metres * scale;
                            let mut line =
                                PathBuilder::stroke(px(1.)).dash_array(&[px(2.), px(5.)]);
                            line.move_to(at(0., y));
                            line.line_to(at(width, y));
                            if let Ok(path) = line.build() {
                                window.paint_path(path, guide);
                            }
                        }
                        let mut centre = PathBuilder::stroke(px(1.)).dash_array(&[px(4.), px(4.)]);
                        centre.move_to(at(middle, base));
                        centre.line_to(at(middle, 0.));
                        if let Ok(path) = centre.build() {
                            window.paint_path(path, guide);
                        }

                        // The head seen from above, its back cut off by the
                        // bottom edge: the eyes sit near its front, and its
                        // nose points the way it faces.
                        let outline = palette::text_4();
                        let radius = 19. * unit;
                        let (hx, hy) = (middle, base + 13. * unit);
                        let k = 0.5523 * radius;
                        let mut head = PathBuilder::stroke(px(1.5));
                        head.move_to(at(hx + radius, hy));
                        head.cubic_bezier_to(
                            at(hx, hy + radius),
                            at(hx + radius, hy + k),
                            at(hx + k, hy + radius),
                        );
                        head.cubic_bezier_to(
                            at(hx - radius, hy),
                            at(hx - k, hy + radius),
                            at(hx - radius, hy + k),
                        );
                        head.cubic_bezier_to(
                            at(hx, hy - radius),
                            at(hx - radius, hy - k),
                            at(hx - k, hy - radius),
                        );
                        head.cubic_bezier_to(
                            at(hx + radius, hy),
                            at(hx + k, hy - radius),
                            at(hx + radius, hy - k),
                        );
                        head.close();
                        if let Ok(path) = head.build() {
                            window.paint_path(path, outline);
                        }
                        let front = hy - radius;
                        let mut nose = PathBuilder::stroke(px(1.5));
                        nose.move_to(at(hx - 3.5 * unit, front + 0.5 * unit));
                        nose.line_to(at(hx, front - 4.5 * unit));
                        nose.line_to(at(hx + 3.5 * unit, front + 0.5 * unit));
                        if let Ok(path) = nose.build() {
                            window.paint_path(path, outline);
                        }

                        let eyes = [
                            middle - HALF_EYE_SEPARATION_M * scale,
                            middle + HALF_EYE_SEPARATION_M * scale,
                        ];
                        // Where the eyes meet, when that's in the drawing.
                        let meeting = output
                            .and_then(|[left, right]| {
                                let distance = meeting_point(
                                    eyes[0],
                                    left[0].to_radians(),
                                    eyes[1],
                                    right[0].to_radians(),
                                )?;
                                Some((eyes[0] + left[0].to_radians().tan() * distance, distance))
                            })
                            .filter(|(_, distance)| *distance < base)
                            .map(|(x, distance)| (x, base - distance));

                        if let Some(output) = output {
                            for (index, x) in eyes.into_iter().enumerate() {
                                // Yaw is positive to the right, so a converging
                                // left eye turns right.
                                let yaw = output[index][0].to_radians();
                                let length = 600. * unit;
                                let end = (x + yaw.sin() * length, base - yaw.cos() * length);
                                let mut ray = |from: (f32, f32), to: (f32, f32), alpha: f32| {
                                    let mut ray = if index == 0 {
                                        PathBuilder::stroke(px(1.5))
                                    } else {
                                        PathBuilder::stroke(px(1.5)).dash_array(&[px(4.), px(3.)])
                                    };
                                    ray.move_to(at(from.0, from.1));
                                    ray.line_to(at(to.0, to.1));
                                    if let Ok(path) = ray.build() {
                                        window.paint_path(path, ink.opacity(alpha));
                                    }
                                };
                                // Past where they meet, the rays only fade on.
                                match meeting {
                                    Some(meet) => {
                                        ray((x, base), meet, 1.);
                                        ray(meet, end, 0.2);
                                    }
                                    None => ray((x, base), end, 1.),
                                }
                            }
                        }
                        // The left eye solid, the right a ring, as their rays.
                        for (index, x) in eyes.into_iter().enumerate() {
                            let mark = Bounds::centered_at(at(x, base), size(px(10.), px(10.)));
                            let quad = if index == 0 {
                                fill(mark, ink).corner_radii(px(5.))
                            } else {
                                fill(mark, palette::sunken())
                                    .corner_radii(px(5.))
                                    .border_widths(px(1.5))
                                    .border_color(ink)
                            };
                            window.paint_quad(quad);
                        }
                        if let Some((x, y)) = meeting {
                            let ring = Bounds::centered_at(at(x, y), size(px(18.), px(18.)));
                            window.paint_quad(
                                fill(ring, ink.opacity(0.))
                                    .corner_radii(px(9.))
                                    .border_widths(px(1.5))
                                    .border_color(ink),
                            );
                            let dot = Bounds::centered_at(at(x, y), size(px(4.), px(4.)));
                            window.paint_quad(fill(dot, ink).corner_radii(px(2.)));
                        }
                    },
                )
                .size_full(),
            )
            .children(labels)
            .child(
                div()
                    .absolute()
                    .right(px(10.))
                    .top(px(8.))
                    .text_size(px(10.))
                    .text_color(palette::text_3())
                    .child(t!("eyes.seen_from_above")),
            )
    }
}

/// The latest eye camera snapshot, or a placeholder when there is none, with
/// the pupils found in it outlined.
#[derive(IntoElement)]
struct SnapshotView {
    image: Option<Arc<RenderImage>>,
    /// The pupils found in `image`, left view then right.
    pupils: Option<[Option<PupilMark>; 2]>,
}

impl RenderOnce for SnapshotView {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let view = div()
            .relative()
            .w_full()
            .aspect_ratio(2.)
            .rounded(px(9.))
            .overflow_hidden()
            .border_1()
            .border_color(palette::line());
        match self.image {
            // Camera snapshots are grayscale raster data on black.
            Some(image) => view
                .bg(black())
                .child(img(image).size_full().object_fit(ObjectFit::Contain))
                .children(self.pupils.map(pupil_outlines)),
            None => view.bg(palette::sunken()).child(EmptyState::new(
                IconName::CameraOff,
                t!("eyes.no_eye_images"),
                t!("eyes.no_eye_images_hint"),
            )),
        }
    }
}

/// Line segments in a pupil's outline.
const PUPIL_OUTLINE_STEPS: usize = 32;

/// Each pupil found outlined over the snapshot, which fills its view: solid
/// where its size is used, dashed while it waits for the other eye to agree.
fn pupil_outlines(pupils: [Option<PupilMark>; 2]) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _| {
            let scale = bounds.size.width.as_f32() / FRAME_WIDTH as f32;
            let origin = bounds.origin;
            let at = |x: f32, y: f32| point(origin.x + px(x), origin.y + px(y));
            for mark in pupils.into_iter().flatten() {
                let (sin, cos) = mark.angle.sin_cos();
                let [a, b] = mark.axes.map(|axis| axis / 2. * scale);
                let [x, y] = mark.centre.map(|value| value * scale);
                let around = |turn: f32| {
                    let (u, v) = (a * turn.cos(), b * turn.sin());
                    at(x + u * cos - v * sin, y + u * sin + v * cos)
                };
                let (width, colour) = if mark.used {
                    (px(1.5), palette::good())
                } else {
                    (px(1.5), palette::signal())
                };
                let mut outline = PathBuilder::stroke(width);
                if !mark.used {
                    outline = outline.dash_array(&[px(3.), px(3.)]);
                }
                outline.move_to(around(0.));
                for step in 1..=PUPIL_OUTLINE_STEPS {
                    outline.line_to(around(
                        step as f32 * std::f32::consts::TAU / PUPIL_OUTLINE_STEPS as f32,
                    ));
                }
                if let Ok(path) = outline.build() {
                    window.paint_path(path, colour);
                }
                let mut centre = PathBuilder::stroke(px(1.));
                centre.move_to(at(x - 3., y));
                centre.line_to(at(x + 3., y));
                centre.move_to(at(x, y - 3.));
                centre.line_to(at(x, y + 3.));
                if let Ok(path) = centre.build() {
                    window.paint_path(path, colour);
                }
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// How far ahead of the eyes, in the drawing's pixels, two gaze rays from
/// `left_x` and `right_x` with the given yaws meet. Yaw is in radians,
/// positive to the right; rays that don't converge in front never meet.
fn meeting_point(left_x: f32, left_yaw: f32, right_x: f32, right_yaw: f32) -> Option<f32> {
    let spread = left_yaw.tan() - right_yaw.tan();
    if spread <= 1e-4 {
        return None;
    }
    Some((right_x - left_x) / spread)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gaze_rays_meet_only_when_they_converge() {
        // Eyes 60 px apart, each turned 10 degrees inward, meet straight
        // ahead at 30 / tan(10°) px.
        let inward = 10f32.to_radians();
        let distance = meeting_point(-30., inward, 30., -inward).unwrap();
        assert!((distance - 30. / inward.tan()).abs() < 0.01);
        // Parallel or diverging rays never meet in front.
        assert_eq!(meeting_point(-30., 0., 30., 0.), None);
        assert_eq!(meeting_point(-30., -inward, 30., inward), None);
    }

    #[test]
    fn each_eye_reads_across_then_up() {
        assert_eq!(eye_angles([8.7, 1.2]), "8.7° right \u{b7} vertical +1.2°");
        assert_eq!(eye_angles([-4.4, -1.0]), "4.4° left \u{b7} vertical -1.0°");
    }
}
