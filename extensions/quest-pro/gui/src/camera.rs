//! Mouth: the live stereo image from the Quest Pro's lower-face cameras, the
//! tongue and cheek puffs VRChat receives from it, and the settings that
//! shape them. With the headset's five-camera stream, the brow camera too.
use crate::daemon::{Settings, SettingsPatch, Status, TongueSource, VisibilityMode};
use crate::live::{CameraFeed, QuestProState};
use crate::pages;
use crate::summary::{self, Connection, Rates, Tone, TongueReading, TongueState};
use crate::tongue::DirectionPad;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    h_flex, v_flex, ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    black, div, img, px, rgb, white, AnyElement, App, AppContext as _, Context, Div, Entity, Hsla,
    InteractiveElement as _, IntoElement, ObjectFit, ParentElement, Render, RenderImage,
    RenderOnce, SharedString, StatefulInteractiveElement as _, Styled, StyledImage as _,
    Subscription, Task, Window,
};
use rust_i18n::t;
use std::borrow::Cow;
use std::cmp::Reverse;
use std::sync::Arc;
use vrft_gui_core::extension::open_page;
use vrft_gui_core::launcher::{LaunchState, Launcher, StartVrft};
use vrft_gui_core::palette;
use vrft_gui_core::widgets::{
    cap, card, mono, ButtonExt as _, EmptyState, Meter, Notice, PageHeader, StatusDot,
};

/// Below this content width the side column moves under the camera image.
const SIDE_BY_SIDE_WIDTH: f32 = 860.;
const PANEL_WIDTH: f32 = 300.;
/// Below this the visibility choices stack in one column.
const OPTION_GRID_WIDTH: f32 = 520.;

/// The ways to decide whether the tongue shows, as people pick them, with a
/// line on when each suits.
fn visibility() -> [(VisibilityMode, Cow<'static, str>, Cow<'static, str>); 4] {
    [
        (
            VisibilityMode::Camera,
            t!("camera.visibility_camera"),
            t!("camera.visibility_camera_about"),
        ),
        (
            VisibilityMode::Weighted,
            t!("camera.visibility_weighted"),
            t!("camera.visibility_weighted_about"),
        ),
        (
            VisibilityMode::Native,
            t!("camera.visibility_native"),
            t!("camera.visibility_native_about"),
        ),
        (
            VisibilityMode::Agreement,
            t!("camera.visibility_agreement"),
            t!("camera.visibility_agreement_about"),
        ),
    ]
}

/// VRFT's twelve tongue expressions, in the order the daemon sends them.
fn sent() -> [Cow<'static, str>; 12] {
    [
        t!("camera.sent_out"),
        t!("camera.sent_up"),
        t!("camera.sent_down"),
        t!("camera.sent_left"),
        t!("camera.sent_right"),
        t!("camera.sent_roll"),
        t!("camera.sent_bend_down"),
        t!("camera.sent_curl_up"),
        t!("camera.sent_squish"),
        t!("camera.sent_flat"),
        t!("camera.sent_twist_left"),
        t!("camera.sent_twist_right"),
    ]
}

/// How many values VRChat receives.
const SENT_COUNT: usize = 12;

/// How many of the values VRChat receives show while the list is folded.
const SENT_FOLDED: usize = 5;

pub struct MouthPage {
    daemon: Entity<QuestProState>,
    camera: Entity<CameraFeed>,
    /// The brow camera, shown while the headset sends all five cameras.
    brow: Entity<CameraFeed>,
    launcher: Entity<Launcher>,
    smoothing: Entity<SliderState>,
    /// The smoothing the daemon last reported, so the slider only follows
    /// changes made elsewhere, not every status.
    shown_smoothing: Option<f32>,
    /// Why saving a setting failed.
    message: Option<Notice>,
    /// Whether "What VRChat receives" shows every value.
    show_sent: bool,
    /// Whether the experimental tongue settings are open.
    show_experimental: bool,
    _save: Option<Task<()>>,
    /// Asking the daemon to try the headset again, while it does.
    retrying: Option<Task<()>>,
    _subscriptions: [Subscription; 5],
}

impl MouthPage {
    pub fn new(
        daemon: Entity<QuestProState>,
        camera: Entity<CameraFeed>,
        brow: Entity<CameraFeed>,
        launcher: Entity<Launcher>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let smoothing = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(100.)
                .step(1.)
                .default_value(Settings::default().tongue_smoothing)
        });
        let subscriptions = [
            cx.observe_in(&daemon, window, |page, _, window, cx| {
                page.follow_smoothing(window, cx);
                cx.notify();
            }),
            cx.observe(&camera, |_, _, cx| cx.notify()),
            cx.observe(&brow, |_, _, cx| cx.notify()),
            cx.observe(&launcher, |_, _, cx| cx.notify()),
            // The number follows the drag; the setting saves on release.
            cx.subscribe(&smoothing, |page, _, event: &SliderEvent, cx| match event {
                SliderEvent::Change(_) => cx.notify(),
                SliderEvent::Release(value) => page.save(
                    SettingsPatch {
                        tongue_smoothing: Some(value.start().round()),
                        ..SettingsPatch::default()
                    },
                    cx,
                ),
            }),
        ];
        Self {
            daemon,
            camera,
            brow,
            launcher,
            smoothing,
            shown_smoothing: None,
            message: None,
            show_sent: false,
            show_experimental: false,
            _save: None,
            retrying: None,
            _subscriptions: subscriptions,
        }
    }

    /// Moves the slider to a smoothing changed elsewhere, such as in the
    /// browser preview.
    fn follow_smoothing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(value) = self
            .daemon
            .read(cx)
            .status()
            .map(|status| status.settings.tongue_smoothing)
        else {
            return;
        };
        if self.shown_smoothing != Some(value) {
            self.shown_smoothing = Some(value);
            self.smoothing
                .update(cx, |slider, cx| slider.set_value(value, window, cx));
        }
    }

    fn set_visibility(&mut self, mode: VisibilityMode, cx: &mut Context<Self>) {
        self.save(
            SettingsPatch {
                tongue_visibility: Some(mode),
                ..SettingsPatch::default()
            },
            cx,
        );
    }

    /// Shows the change straight away, then saves it; only a failure is
    /// reported.
    fn save(&mut self, patch: SettingsPatch, cx: &mut Context<Self>) {
        let Some(mut settings) = self
            .daemon
            .read(cx)
            .status()
            .map(|status| status.settings.clone())
        else {
            return;
        };
        if let Some(smoothing) = patch.tongue_smoothing {
            settings.tongue_smoothing = smoothing;
            self.shown_smoothing = Some(smoothing);
        }
        if let Some(mode) = patch.tongue_visibility {
            settings.tongue_visibility = mode;
        }
        if let Some(cheeks) = patch.cheek_puffs {
            settings.cheek_puffs = cheeks;
        }
        if let Some(on) = patch.mouth_model {
            settings.mouth_model = on;
        }
        self.daemon
            .update(cx, |daemon, cx| daemon.show_settings(settings, cx));
        self.message = None;
        let client = self.daemon.read(cx).client();
        self._save = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { client.update_settings(&patch) })
                .await;
            this.update(cx, |page, cx| {
                match result {
                    Ok(settings) => page
                        .daemon
                        .update(cx, |daemon, cx| daemon.show_settings(settings, cx)),
                    Err(error) => {
                        page.message = Some(Notice::error(&t!("camera.couldnt_save"), &error))
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Tries the headset again now instead of waiting out the daemon's backoff.
    fn retry(&mut self, cx: &mut Context<Self>) {
        let client = self.daemon.read(cx).client();
        self.message = None;
        self.retrying = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { client.reconnect() })
                .await;
            this.update(cx, |page, cx| {
                page.retrying = None;
                if let Err(error) = result {
                    page.message = Some(Notice::error(&t!("camera.couldnt_retry"), &error));
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// How smooth the tongue is, and when it shows.
    fn settings_panel(
        &self,
        status: Option<&Status>,
        column_width: f32,
        cx: &Context<Self>,
    ) -> AnyElement {
        let settings = status.map(|status| &status.settings);
        // The slider's own value, so the number moves while it's dragged.
        let smoothing = self.smoothing.read(cx).value().start();
        let mode = settings.map(|settings| settings.tongue_visibility);
        // With the mouth model off, none of these are used.
        let model_off = settings.is_some_and(|settings| !settings.mouth_model);
        let disabled = status.is_none() || model_off;
        let experimental =
            self.show_experimental || mode.is_some_and(|mode| mode != VisibilityMode::default());
        let options =
            visibility()
                .into_iter()
                .enumerate()
                .map(|(index, (choice, title, about))| {
                    let selected = mode == Some(choice);
                    h_flex()
                        .id(("tongue-visibility", index))
                        .min_w_0()
                        .gap(px(10.))
                        .items_start()
                        .px_3()
                        .py(px(11.))
                        .rounded(px(10.))
                        .border_1()
                        .border_color(if selected {
                            palette::line_focus()
                        } else {
                            option_line()
                        })
                        .bg(if selected {
                            palette::raised()
                        } else {
                            palette::rail()
                        })
                        .when(disabled, |option| option.opacity(0.5))
                        .when(!disabled && !selected, |option| {
                            option
                                .cursor_pointer()
                                .hover(|style| style.border_color(option_hover_line()))
                                .on_click(cx.listener(move |page, _, _, cx| {
                                    page.set_visibility(choice, cx)
                                }))
                        })
                        .child(radio_mark(selected))
                        .child(
                            v_flex()
                                .min_w_0()
                                .gap_0p5()
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .when(selected, |title| title.font_medium())
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .text_color(palette::text_3())
                                        .child(about),
                                ),
                        )
                });
        card(cx)
            .flex()
            .flex_col()
            .gap(px(14.))
            .px(px(18.))
            .pt_4()
            .pb(px(18.))
            .child(card_title(t!("camera.tongue_settings")))
            .when(model_off, |card| {
                card.child(
                    div()
                        .text_size(px(12.5))
                        .text_color(palette::text_3())
                        .child(t!("camera.mouth_model_off")),
                )
            })
            .child(
                h_flex()
                    .gap(px(14.))
                    .child(
                        h_flex()
                            .id("smoothing-label")
                            .w(px(170.))
                            .flex_none()
                            .gap_1p5()
                            .text_size(px(13.))
                            .text_color(soft_text())
                            .child(t!("camera.smoothing"))
                            .child(
                                div()
                                    .text_color(palette::text_4())
                                    .child(Icon::new(IconName::Info).size(px(13.))),
                            )
                            .tooltip(|window, cx| {
                                Tooltip::new(SharedString::from(t!("camera.smoothing_note")))
                                    .build(window, cx)
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Slider::new(&self.smoothing).disabled(disabled)),
                    )
                    .child(
                        mono(format!("{smoothing:.0}"))
                            .w(px(40.))
                            .flex_none()
                            .text_right()
                            .text_size(px(12.5))
                            .text_color(soft_text()),
                    ),
            )
            // Other ways to decide the tongue is out, tucked away; open
            // while one of them is picked, so it can be seen and undone.
            .child(
                h_flex().child(
                    Button::new("toggle-experimental")
                        .ghost()
                        .small()
                        .ml(px(-8.))
                        .label(t!("camera.experimental"))
                        .child(
                            Icon::new(if experimental {
                                IconName::ChevronUp
                            } else {
                                IconName::ChevronDown
                            })
                            .size(px(13.)),
                        )
                        .on_click(cx.listener(move |page, _, _, cx| {
                            page.show_experimental = !experimental;
                            cx.notify();
                        })),
                ),
            )
            .when(experimental, |card| {
                card.child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(soft_text())
                                .child(t!("camera.when_to_show")),
                        )
                        .child(
                            div()
                                .grid()
                                .grid_cols(if column_width >= OPTION_GRID_WIDTH {
                                    2
                                } else {
                                    1
                                })
                                .gap_2()
                                .children(options),
                        ),
                )
            })
            .children(self.message.clone())
            .into_any_element()
    }

    /// Why the tongue model isn't the source, and where to fix it.
    fn model_notice(&self, status: Option<&Status>, cx: &Context<Self>) -> Option<AnyElement> {
        status?;
        let reading = summary::tongue_model(status);
        // Idle waits on the cameras, which the camera card already says.
        if matches!(reading.tone, Tone::Good | Tone::Off) {
            return None;
        }
        let text = if reading.detail.is_empty() {
            t!("camera.tongue_model", state = reading.value)
        } else {
            t!(
                "camera.tongue_model_detail",
                state = reading.value,
                detail = reading.detail
            )
        };
        // "Comes from", beside it, says what VRChat gets meanwhile.
        Some(
            v_flex()
                .gap_2()
                .items_start()
                .child(Notice::new(reading.tone, text))
                .child(
                    Button::new("open-training")
                        .regular()
                        .icon(IconName::Sparkles)
                        .label(t!("camera.open_training"))
                        .on_click(cx.listener(|_, _, _, cx| open_page(pages::TRAINING, cx))),
                )
                .into_any_element(),
        )
    }

    /// Frame rate, frame age, model time and skipped frames, under the image.
    fn stream_stats(&self, status: Option<&Status>, rates: &Rates) -> impl IntoElement {
        let model = status.and_then(|status| status.model.as_ref());
        let cells: [(Cow<'static, str>, String); 4] = [
            (
                t!("camera.frame_rate"),
                rates
                    .camera_fps
                    .filter(|_| status.is_some_and(summary::camera_live))
                    .map(|fps| t!("camera.fps", fps = format!("{fps:.0}")).into_owned())
                    .unwrap_or_else(|| "\u{2014}".into()),
            ),
            (
                t!("camera.frame_age"),
                status
                    .and_then(|status| status.frame_age_ms)
                    .map(|age| t!("camera.milliseconds", ms = age).into_owned())
                    .unwrap_or_else(|| "\u{2014}".into()),
            ),
            (
                t!("camera.model"),
                model
                    .filter(|model| model.fresh)
                    .map(|model| {
                        t!(
                            "camera.milliseconds",
                            ms = format!("{:.1}", model.inference_ms)
                        )
                        .into_owned()
                    })
                    .unwrap_or_else(|| "\u{2014}".into()),
            ),
            (
                t!("camera.skipped"),
                model
                    .map(|model| model.skipped_frames.to_string())
                    .unwrap_or_else(|| "\u{2014}".into()),
            ),
        ];
        div()
            .grid()
            .grid_cols(4)
            .border_t_1()
            .border_color(palette::line())
            .children(
                cells
                    .into_iter()
                    .enumerate()
                    .map(|(index, (label, value))| {
                        v_flex()
                            .min_w_0()
                            .gap_0p5()
                            .px_4()
                            .py(px(10.))
                            .when(index > 0, |cell| {
                                cell.border_l_1().border_color(palette::line_soft())
                            })
                            .child(cap(label))
                            .child(mono(value).text_size(px(13.)).truncate())
                    }),
            )
    }

    /// Each cheek's puff as VRChat receives it, where it comes from, and the
    /// switch that takes it from the cameras.
    /// The brow camera (camera 4), while the headset sends all five cameras.
    fn brow_panel(&self, status: Option<&Status>, cx: &Context<Self>) -> Option<AnyElement> {
        if !status.is_some_and(|status| status.five_cameras) {
            return None;
        }
        let image = self.brow.read(cx).image();
        // Inside the card's border, so a pixel tighter than its corners.
        let corner = cx.theme().radius_lg - px(1.);
        Some(
            card(cx)
                .overflow_hidden()
                .child(
                    div()
                        .relative()
                        .w_full()
                        .aspect_ratio(1.)
                        .rounded_t(corner)
                        .overflow_hidden()
                        // Grayscale raster data, black in either theme.
                        .bg(black())
                        .when_some(image, |this, image| {
                            this.child(
                                img(image)
                                    .size_full()
                                    .rounded_t(corner)
                                    .object_fit(ObjectFit::Contain),
                            )
                        }),
                )
                .child(
                    v_flex()
                        .gap_1()
                        .px(px(18.))
                        .py_3()
                        .border_t_1()
                        .border_color(palette::line())
                        .child(card_title(t!("camera.brow_camera")))
                        .child(
                            div()
                                .text_xs()
                                .text_color(palette::text_3())
                                .child(t!("camera.brow_camera_hint")),
                        ),
                )
                .into_any_element(),
        )
    }

    fn cheek_panel(&self, status: Option<&Status>, cx: &Context<Self>) -> AnyElement {
        let output = status.and_then(|status| status.output.as_ref());
        let checked = status.map_or(Settings::default().cheek_puffs, |status| {
            status.settings.cheek_puffs
        });
        let puffs = output.map(|output| output.cheek_puffs);
        let source = output.map(|output| output.cheek_source);
        // On, with the model running, yet the headset's: the model never
        // learned them.
        let unlearned = checked
            && source == Some(TongueSource::TrackingModule)
            && status
                .and_then(|status| status.model.as_ref())
                .is_some_and(|model| model.fresh);
        card(cx)
            .flex()
            .flex_col()
            .gap_2()
            .px(px(18.))
            .pt(px(14.))
            .pb_3()
            .child(
                h_flex()
                    .items_start()
                    .justify_between()
                    .gap_3()
                    .child(
                        v_flex()
                            .min_w_0()
                            .gap_1()
                            .child(card_title(t!("camera.cheek_puffs")))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(palette::text_3())
                                    .child(t!("camera.cheek_puffs_hint")),
                            ),
                    )
                    .child(
                        div().flex_none().mt(px(1.)).child(
                            Switch::new("cheek-puffs")
                                .accessibility_label(t!("camera.cheek_puffs"))
                                .checked(checked)
                                .disabled(
                                    status.is_none()
                                        || !status
                                            .is_some_and(|status| status.settings.mouth_model),
                                )
                                .on_change(cx.listener(|page, checked: &bool, _, cx| {
                                    page.save(
                                        SettingsPatch {
                                            cheek_puffs: Some(*checked),
                                            ..SettingsPatch::default()
                                        },
                                        cx,
                                    )
                                })),
                        ),
                    ),
            )
            .child(
                v_flex()
                    .child(sent_row(t!("camera.cheek_left"), puffs.map(|p| p[0])))
                    .child(sent_row(t!("camera.cheek_right"), puffs.map(|p| p[1]))),
            )
            .child(
                ruled_row(t!("camera.comes_from")).child(div().flex_none().child(match source {
                    Some(TongueSource::EnhancedModel) => t!("camera.source_cameras"),
                    Some(TongueSource::TrackingModule) => t!("camera.source_headset"),
                    None => "\u{2014}".into(),
                })),
            )
            .when(unlearned, |card| {
                card.child(
                    div()
                        .text_xs()
                        .text_color(palette::text_3())
                        .child(t!("camera.cheeks_unlearned")),
                )
            })
            .into_any_element()
    }

    /// Every tongue value VRChat receives, the strongest few until asked for
    /// all of them, and then the two visibility readings behind whether it
    /// shows.
    fn sent_panel(&self, status: Option<&Status>, cx: &Context<Self>) -> AnyElement {
        let output = status.and_then(|status| status.output.as_ref());
        let toggle = Button::new("toggle-sent")
            .ghost()
            .small()
            .mr(px(-8.))
            .label(if self.show_sent {
                t!("camera.fewer")
            } else {
                t!("camera.all", count = SENT_COUNT)
            })
            .child(
                Icon::new(if self.show_sent {
                    IconName::ChevronUp
                } else {
                    IconName::ChevronDown
                })
                .size(px(13.)),
            )
            .on_click(cx.listener(|page, _, _, cx| {
                page.show_sent = !page.show_sent;
                cx.notify();
            }));
        let values = output.map(|output| output.values);
        let shown: Vec<usize> = if self.show_sent {
            (0..SENT_COUNT).collect()
        } else {
            strongest(values.as_ref(), SENT_FOLDED)
        };
        let names = sent();
        let panel =
            card(cx)
                .flex()
                .flex_col()
                .gap_2()
                .px(px(18.))
                .pt(px(14.))
                .pb_3()
                .child(
                    h_flex()
                        .justify_between()
                        .gap_3()
                        .child(card_title(t!("camera.what_vrchat_receives")))
                        .child(toggle),
                )
                .child(v_flex().children(shown.into_iter().map(|index| {
                    sent_row(names[index].clone(), values.map(|values| values[index]))
                })));
        if !self.show_sent {
            return panel.into_any_element();
        }
        let camera = status
            .and_then(|status| status.model.as_ref())
            .filter(|model| model.fresh)
            .map(|model| model.values[0]);
        let percent = |value: Option<f32>| {
            value.map_or_else(
                || "\u{2014}".to_string(),
                |value| format!("{:.0}%", value * 100.),
            )
        };
        panel
            .child(
                v_flex()
                    .mt_1()
                    .child(ruled_row(t!("camera.cameras_see_tongue")).child(mono(percent(camera))))
                    .child(
                        ruled_row(t!("camera.headset_sees_tongue")).child(mono(percent(
                            output.and_then(|output| output.native_tongue_out),
                        ))),
                    )
                    .child(ruled_row(t!("camera.combined")).child(mono(percent(
                        output.and_then(|output| output.fused_visibility),
                    )))),
            )
            .into_any_element()
    }
}

impl Render for MouthPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.daemon.read(cx);
        let status = state.status().cloned();
        let status = status.as_ref();
        let rates = state.rates();
        let starting = *self.launcher.read(cx).state() == LaunchState::Starting;
        let screen = screen_state(state.connection(), status, &rates, starting);
        let image = self.camera.read(cx).image();
        let start = matches!(screen, Screen::Offline)
            .then(|| StartVrft::new(self.launcher.clone()).into_any_element());
        let restart = matches!(screen, Screen::Inactive).then(|| {
            let launcher = self.launcher.clone();
            Button::new("restart-vrft")
                .primary()
                .regular()
                .icon(IconName::RotateCcw)
                .label(t!("camera.restart_vrft"))
                .on_click(move |_, _, cx| launcher.update(cx, |launcher, cx| launcher.restart(cx)))
                .into_any_element()
        });
        // With the headset not streaming, the fix is on the Headset page,
        // or it may just need trying again sooner than the daemon would.
        let backing_off = status.is_some_and(|status| status.retry_in_ms.is_some());
        let setup = matches!(&screen, Screen::Waiting { headset: false, .. }).then(|| {
            h_flex()
                .gap_2()
                .when(backing_off, |row| {
                    row.child(
                        Button::new("retry-headset")
                            .primary()
                            .regular()
                            .icon(IconName::RotateCcw)
                            .label(t!("camera.retry"))
                            .disabled(self.retrying.is_some())
                            .on_click(cx.listener(|page, _, _, cx| page.retry(cx))),
                    )
                })
                .child(
                    Button::new("open-headset")
                        .regular()
                        .icon(IconName::Glasses)
                        .label(t!("camera.open_headset"))
                        .on_click(cx.listener(|_, _, _, cx| open_page(pages::HEADSET, cx))),
                )
                .into_any_element()
        });
        // Otherwise the camera card says how it stands.
        let pill = match &screen {
            Screen::Live { fps } => Some(live_pill(*fps)),
            _ => None,
        };
        // Whether the cameras track the tongue and cheeks, or the headset
        // does, beside the header.
        let model_on = status.is_none_or(|status| status.settings.mouth_model);
        let model_switch = h_flex()
            .id("mouth-model")
            .flex_none()
            .gap_2()
            .items_center()
            .text_size(px(13.))
            .text_color(soft_text())
            .child(t!("camera.mouth_model"))
            .child(
                Switch::new("mouth-model-switch")
                    .checked(model_on)
                    .disabled(status.is_none())
                    .accessibility_label(t!("camera.mouth_model"))
                    .on_change(cx.listener(|page, checked: &bool, _, cx| {
                        page.save(
                            SettingsPatch {
                                mouth_model: Some(*checked),
                                ..SettingsPatch::default()
                            },
                            cx,
                        )
                    })),
            )
            .tooltip(|window, cx| {
                Tooltip::new(SharedString::from(t!("camera.mouth_model_tooltip"))).build(window, cx)
            });
        let trailing = h_flex()
            .gap_4()
            .items_center()
            .child(model_switch)
            .children(pill);
        let content = vrft_gui_core::content_width(window);
        let wide = content >= SIDE_BY_SIDE_WIDTH;
        let column = if wide {
            content - PANEL_WIDTH - 20.
        } else {
            content
        };
        let model_notice = self.model_notice(status, cx);
        let settings = self.settings_panel(status, column, cx);
        let stats = self.stream_stats(status, &rates);
        let source = status
            .and_then(|status| status.output.as_ref())
            .map(|output| output.source);
        let tongue = tongue_panel(tongue_reading(status, &rates), source, cx);
        let sent = self.sent_panel(status, cx);
        let cheeks = self.cheek_panel(status, cx);
        let brow = self.brow_panel(status, cx);

        v_flex()
            .gap(px(20.))
            .child(
                PageHeader::new(t!("camera.page_title"))
                    .description(t!("camera.page_description"))
                    .trailing(trailing),
            )
            .child(
                div()
                    .flex()
                    .gap(px(20.))
                    .map(|this| {
                        if wide {
                            this.flex_row().items_start()
                        } else {
                            this.flex_col()
                        }
                    })
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_4()
                            .child(
                                card(cx)
                                    .overflow_hidden()
                                    .child(CameraScreen {
                                        image,
                                        screen,
                                        action: start.or(restart).or(setup),
                                    })
                                    .child(stats),
                            )
                            .children(model_notice)
                            .child(settings),
                    )
                    .child(
                        v_flex()
                            .flex_none()
                            .gap_4()
                            .when(wide, |this| this.w(px(PANEL_WIDTH)))
                            .child(tongue)
                            .child(sent)
                            .child(cheeks)
                            .children(brow),
                    ),
            )
    }
}

enum Screen {
    Live {
        fps: Option<f32>,
    },
    Waiting {
        title: SharedString,
        detail: SharedString,
        /// Whether the headset is streaming, just without camera frames.
        headset: bool,
    },
    /// VRFT runs, but without its Quest Pro support.
    Inactive,
    Offline,
}

fn screen_state(
    connection: &Connection,
    status: Option<&Status>,
    rates: &Rates,
    starting: bool,
) -> Screen {
    match (connection, status) {
        (Connection::Online, Some(status)) if summary::camera_live(status) => Screen::Live {
            fps: rates.camera_fps,
        },
        // A headset app VRFT can't read says which side to update; one that's
        // connected just needs the headset's cameras running.
        (Connection::Online, Some(status)) if status.headset_mismatch.is_some() => {
            let headset = summary::headset(Some(status));
            Screen::Waiting {
                title: headset.value.into(),
                detail: headset.detail.into(),
                headset: false,
            }
        }
        (Connection::Online, Some(status)) => Screen::Waiting {
            title: t!("camera.waiting_title").into(),
            detail: if status.source.is_some() {
                t!("camera.waiting_detail").into()
            } else {
                let state = summary::first_sentence(&status.status);
                match status.retry_in_ms {
                    Some(ms) => t!(
                        "camera.retrying_in",
                        state = state,
                        seconds = ms.div_ceil(1000)
                    )
                    .into_owned(),
                    None => state.to_string(),
                }
                .into()
            },
            headset: status.source.is_some(),
        },
        (Connection::Online, None) => Screen::Inactive,
        (Connection::Connecting, _) => Screen::Waiting {
            title: t!("camera.connecting").into(),
            detail: SharedString::default(),
            headset: true,
        },
        _ if starting => Screen::Waiting {
            title: t!("camera.starting").into(),
            detail: SharedString::default(),
            headset: true,
        },
        _ => Screen::Offline,
    }
}

/// The stereo camera image, or what is keeping it from showing. It sits at
/// the top of the camera card, which clips its corners.
#[derive(IntoElement)]
struct CameraScreen {
    image: Option<Arc<RenderImage>>,
    screen: Screen,
    /// What to do about a missing picture, such as starting VRFT.
    action: Option<AnyElement>,
}

impl RenderOnce for CameraScreen {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        // Inside the card's border, so a pixel tighter than its corners.
        let corner = cx.theme().radius_lg - px(1.);
        let screen = div()
            .relative()
            .w_full()
            .aspect_ratio(2.)
            .rounded_t(corner)
            .overflow_hidden();
        let (icon, title, detail) = match self.screen {
            // The camera image is grayscale raster data; its letterbox stays
            // black in either theme.
            Screen::Live { .. } => {
                return screen
                    .bg(black())
                    .when_some(self.image, |this, image| {
                        // Each camera takes one half of the stereo frame.
                        this.child(
                            img(image)
                                .size_full()
                                .rounded_t(corner)
                                .object_fit(ObjectFit::Contain),
                        )
                        .child(camera_side(t!("camera.side_left")).left_3())
                        .child(camera_side(t!("camera.side_right")).right_3())
                    })
                    .child(live_badge());
            }
            Screen::Waiting { title, detail, .. } => (IconName::CameraOff, title, detail),
            // The Restart VRFT button under it says what to do.
            Screen::Inactive => (
                IconName::Puzzle,
                t!("camera.support_not_running").into(),
                SharedString::default(),
            ),
            Screen::Offline => (
                IconName::Unplug,
                t!("camera.vrft_not_running").into(),
                SharedString::default(),
            ),
        };
        // Without a live stream, say so in words rather than showing a stale
        // or empty picture.
        screen
            .bg(palette::sunken())
            .child(EmptyState::new(icon, title, detail).action(self.action))
    }
}

/// "LIVE" over the corner of a camera image, which is always dark, so the
/// badge is too. The page header gives the frame rate.
fn live_badge() -> impl IntoElement {
    h_flex()
        .absolute()
        .top_3()
        .left_3()
        .gap(px(7.))
        .h(px(24.))
        .px(px(9.))
        .rounded(px(6.))
        .bg(black().opacity(0.6))
        .border_1()
        .border_color(white().opacity(0.18))
        .child(div().size(px(6.)).rounded_full().bg(palette::text()))
        .child(cap(t!("camera.live_badge")).text_color(palette::text()))
}

/// Which camera a half of the stereo image comes from, at its bottom corner.
fn camera_side(side: impl Into<SharedString>) -> Div {
    cap(side)
        .absolute()
        .bottom(px(10.))
        .text_color(palette::text_2())
}

/// The header's mark that the cameras are live, with their frame rate.
fn live_pill(fps: Option<f32>) -> impl IntoElement {
    h_flex()
        .flex_none()
        .gap_2()
        .h(px(28.))
        .px_3()
        .rounded_full()
        .border_1()
        .border_color(palette::line_strong())
        .text_size(px(12.5))
        .text_color(palette::text())
        .child(StatusDot::new(Tone::Good))
        .child(t!("camera.live"))
        .children(fps.map(|fps| {
            mono(t!("camera.fps", fps = format!("{fps:.0}"))).text_color(palette::text_3())
        }))
}

/// A card's title, in the size every card on the page shares.
fn card_title(title: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(14.))
        .font_semibold()
        .text_color(palette::text())
        .child(title.into())
}

/// A row under a rule: a quiet label taking the room, then whatever is added.
fn ruled_row(label: impl Into<SharedString>) -> Div {
    h_flex()
        .h(px(32.))
        .gap(px(10.))
        .border_t_1()
        .border_color(palette::line_soft())
        .text_size(px(12.5))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(palette::text_3())
                .child(label.into()),
        )
}

/// One value VRChat receives: its name, a bar and the value.
fn sent_row(name: impl Into<SharedString>, value: Option<f32>) -> impl IntoElement {
    let quiet = value.is_none_or(|value| value < 0.005);
    h_flex()
        .h(px(24.))
        .gap(px(10.))
        .text_size(px(12.))
        .child(
            div()
                .w(px(78.))
                .flex_none()
                .truncate()
                .text_color(if quiet {
                    palette::text_3()
                } else {
                    palette::text_2()
                })
                .child(name.into()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(Meter::new(value.unwrap_or(0.), palette::text())),
        )
        .child(
            mono(unit(value))
                .w(px(34.))
                .flex_none()
                .text_right()
                .when(quiet, |value| value.text_color(palette::text_3())),
        )
}

/// The indexes of the `count` strongest values, strongest first, as they
/// read to two places; ties keep the daemon's order. Without values, the
/// first few.
fn strongest(values: Option<&[f32; 12]>, count: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..SENT_COUNT).collect();
    if let Some(values) = values {
        order.sort_by_key(|index| Reverse((values[*index].clamp(0., 1.) * 100.).round() as i32));
    }
    order.truncate(count);
    order
}

/// A value from 0 to 1 as it reads beside a bar: ".64", "0" or "1".
fn unit(value: Option<f32>) -> String {
    let Some(value) = value else {
        return "\u{2014}".into();
    };
    let value = value.clamp(0., 1.);
    if value < 0.005 {
        "0".into()
    } else if value >= 0.995 {
        "1".into()
    } else {
        format!("{value:.2}").trim_start_matches('0').to_string()
    }
}

/// A radio choice's mark: a white ring when chosen, a grey one when not.
fn radio_mark(selected: bool) -> Div {
    let mark = div().flex_none().mt(px(2.)).size(px(14.)).rounded_full();
    if selected {
        mark.flex()
            .items_center()
            .justify_center()
            .bg(palette::text())
            .child(div().size(px(6.)).rounded_full().bg(palette::raised()))
    } else {
        mark.border_1().border_color(palette::text_4())
    }
}

/// Labels beside controls: between the text and its secondary grey.
fn soft_text() -> Hsla {
    rgb(0xd4d4d4).into()
}

/// The edge of a choice that isn't chosen.
fn option_line() -> Hsla {
    rgb(0x26262b).into()
}

fn option_hover_line() -> Hsla {
    rgb(0x3a3a41).into()
}

/// The tongue VRChat is getting from the daemon, or not tracked when there's
/// no status.
pub fn tongue_reading(status: Option<&Status>, rates: &Rates) -> TongueReading {
    status
        .map(|status| summary::tongue(status, rates.tracking()))
        .unwrap_or(TongueReading {
            state: TongueState::NotTracked,
            out: 0.,
            horizontal: 0.,
            vertical: 0.,
        })
}

/// The live tongue: where it points, how far out it is, and where VRChat's
/// tongue comes from. A card; callers may add a line under it.
pub fn tongue_panel(reading: TongueReading, source: Option<TongueSource>, cx: &App) -> Div {
    let source = match source {
        Some(TongueSource::EnhancedModel) => t!("camera.source_cameras"),
        Some(TongueSource::TrackingModule) => t!("camera.source_headset"),
        None => "\u{2014}".into(),
    };
    let (out, far) = match reading.state {
        TongueState::NotTracked => (0., "\u{2014}".to_string()),
        _ => (reading.out, format!("{:.0}%", reading.out * 100.)),
    };
    card(cx)
        .flex()
        .flex_col()
        .gap_3()
        .px(px(18.))
        .pt_4()
        .pb_3()
        .child(
            h_flex()
                .justify_between()
                .gap_3()
                .child(card_title(t!("camera.your_tongue")))
                .child(cap(t!("camera.mirror_view"))),
        )
        .child(
            div()
                .flex()
                .justify_center()
                .py_1()
                .child(tongue_pad(reading)),
        )
        .child(
            v_flex()
                .child(
                    ruled_row(t!("camera.how_far_out"))
                        .child(
                            div()
                                .w(px(80.))
                                .flex_none()
                                .child(Meter::new(out, palette::text())),
                        )
                        .child(mono(far).w(px(34.)).flex_none().text_right()),
                )
                .child(ruled_row(t!("camera.comes_from")).child(div().flex_none().child(source))),
        )
}

const PAD_SIZE: f32 = 184.;

/// Tongue direction as seen in a mirror: your right is on the right. The dot
/// grows the further out the tongue is.
fn tongue_pad(reading: TongueReading) -> DirectionPad {
    let pad = DirectionPad::new(PAD_SIZE);
    match reading.state {
        TongueState::Out => pad.dot(
            reading.horizontal,
            reading.vertical,
            14. + 14. * reading.out,
        ),
        TongueState::In => pad.caption(t!("camera.tongue_in")),
        TongueState::NotTracked => pad.caption(t!("camera.not_tracked")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_read_as_two_places_without_the_leading_zero() {
        assert_eq!(unit(None), "\u{2014}");
        assert_eq!(unit(Some(0.)), "0");
        assert_eq!(unit(Some(0.004)), "0");
        assert_eq!(unit(Some(0.64)), ".64");
        assert_eq!(unit(Some(0.05)), ".05");
        assert_eq!(unit(Some(1.2)), "1");
    }

    #[test]
    fn the_folded_list_shows_the_strongest_values_first() {
        let mut values = [0.; 12];
        values[7] = 0.05;
        values[0] = 0.64;
        values[4] = 0.08;
        values[1] = 0.12;
        assert_eq!(strongest(Some(&values), 5), vec![0, 1, 4, 7, 2]);
        assert_eq!(strongest(None, 5), vec![0, 1, 2, 3, 4]);
    }
}
