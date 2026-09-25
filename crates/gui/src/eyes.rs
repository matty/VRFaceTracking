//! Eyes: independent per-eye gaze from the Quest Pro, recentering, and the
//! settings that decide what VRFT sends for each eye.
use crate::daemon::{Settings, Status};
use crate::launcher::{Launcher, StartVrft};
use crate::live::{CameraFeed, DaemonState};
use crate::summary::Connection;
use crate::summary::{self, Tone};
use crate::widgets::{info_row, Notice, PageHeader, Panel, StatusPill};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    h_flex, v_flex, ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    black, canvas, div, fill, img, point, px, relative, size, AnyElement, App, Bounds, Context,
    Entity, Hsla, IntoElement, ObjectFit, ParentElement, PathBuilder, Pixels, Render, RenderImage,
    RenderOnce, SharedString, Styled, StyledImage as _, Subscription, Task, Window,
};
use std::sync::Arc;
use std::time::Duration;

/// Below this content width the panels stack in one column.
const TWO_COLUMN_WIDTH: f32 = 760.;
/// An eye snapshot older than this is not shown, as in the preview page.
const SNAPSHOT_STALE_MS: u64 = 3000;
const RECENTER_COUNTDOWN: u8 = 3;

enum Recenter {
    Idle,
    Counting(u8),
    Saving,
}

pub struct EyesPage {
    daemon: Entity<DaemonState>,
    snapshots: Entity<CameraFeed>,
    launcher: Entity<Launcher>,
    recenter: Recenter,
    /// The outcome of the last recenter or settings change.
    message: Option<(Tone, SharedString)>,
    _action: Option<Task<()>>,
    _subscriptions: [Subscription; 3],
}

impl EyesPage {
    pub fn new(
        daemon: Entity<DaemonState>,
        snapshots: Entity<CameraFeed>,
        launcher: Entity<Launcher>,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = [
            cx.observe(&daemon, |_, _, cx| cx.notify()),
            cx.observe(&snapshots, |_, _, cx| cx.notify()),
            cx.observe(&launcher, |_, _, cx| cx.notify()),
        ];
        Self {
            daemon,
            snapshots,
            launcher,
            recenter: Recenter::Idle,
            message: None,
            _action: None,
            _subscriptions: subscriptions,
        }
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
                page.finish(
                    result,
                    Some("Recentered. Your avatar now looks straight ahead when you do."),
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
            Some("Recenter undone."),
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
            _ => return,
        }
        self.daemon
            .update(cx, |daemon, cx| daemon.show_settings(settings, cx));
        let client = self.daemon.read(cx).client();
        let patch = serde_json::json!({ key: value });
        self.run(move || client.update_settings(patch), None, cx);
    }

    fn run(
        &mut self,
        request: impl FnOnce() -> anyhow::Result<Settings> + Send + 'static,
        success: Option<&'static str>,
        cx: &mut Context<Self>,
    ) {
        self.message = None;
        self._action = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { request() })
                .await;
            this.update(cx, |page, cx| page.finish(result, success, cx))
                .ok();
        }));
    }

    fn finish(
        &mut self,
        result: anyhow::Result<Settings>,
        success: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        self.message = match result {
            Ok(settings) => {
                self.daemon
                    .update(cx, |daemon, cx| daemon.show_settings(settings, cx));
                success.map(|text| (Tone::Good, text.to_string().into()))
            }
            Err(error) => Some((Tone::Problem, format!("Couldn't save: {error:#}").into())),
        };
        cx.notify();
    }

    fn recenter_panel(&self, status: Option<&Status>, cx: &mut Context<Self>) -> AnyElement {
        let settings = status.map(|status| &status.settings);
        let online = status.is_some();
        let recentered = settings.is_some_and(|settings| settings.eye_offsets.is_some());
        let label: SharedString = match self.recenter {
            Recenter::Idle => "Recenter".into(),
            Recenter::Counting(remaining) => format!("Look ahead… {remaining}").into(),
            Recenter::Saving => "Recentering".into(),
        };
        let theme = cx.theme();
        Panel::new("Recenter")
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(summary::eye_message(status)),
            )
            .when(
                matches!(
                    self.daemon.read(cx).connection(),
                    Connection::Offline { .. }
                ),
                |this| this.child(StartVrft::new(self.launcher.clone())),
            )
            .child(
                h_flex()
                    .gap_4()
                    .items_center()
                    .flex_wrap()
                    .child(
                        Button::new("recenter")
                            .primary()
                            .label(label)
                            .loading(matches!(self.recenter, Recenter::Saving))
                            .disabled(!online || !matches!(self.recenter, Recenter::Idle))
                            .on_click(cx.listener(|page, _, _, cx| page.start_recenter(cx))),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(220.))
                            .gap_0p5()
                            .text_sm()
                            .child(
                                div()
                                    .font_medium()
                                    .child("Look straight ahead at something far away."),
                            )
                            .child(div().text_color(theme.muted_foreground).child(format!(
                                "You'll have {RECENTER_COUNTDOWN} seconds after pressing Recenter to look away from this window."
                            ))),
                    )
                    .when(recentered, |this| {
                        this.child(
                            Button::new("undo-recenter")
                                .ghost()
                                .small()
                                .label("Undo recenter")
                                .disabled(!matches!(self.recenter, Recenter::Idle))
                                .on_click(cx.listener(|page, _, _, cx| page.undo_recenter(cx))),
                        )
                    }),
            )
            .when_some(self.message.clone(), |this, (tone, message)| {
                this.child(Notice::new(tone, message))
            })
            .child(div().h(px(1.)).bg(theme.border))
            .child(
                Switch::new("eye-gaze")
                    .label("Track each eye separately, so your avatar's eyes can cross")
                    .checked(settings.is_some_and(|settings| settings.eye_gaze))
                    .disabled(!online)
                    .on_change(cx.listener(|page, checked: &bool, _, cx| {
                        page.change("eye_gaze", *checked, cx)
                    })),
            )
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
        let two_columns = crate::shell::content_width(window) >= TWO_COLUMN_WIDTH;
        let recenter = self.recenter_panel(status, cx);
        let theme = cx.theme();
        let (left_color, right_color) = (theme.blue, theme.yellow);

        let gaze_rows = match output {
            Some([left, right]) => vec![
                info_row(
                    "Left eye",
                    format!(
                        "{} / {}",
                        summary::signed_degrees(left[0]),
                        summary::signed_degrees(left[1])
                    ),
                    cx,
                ),
                info_row(
                    "Right eye",
                    format!(
                        "{} / {}",
                        summary::signed_degrees(right[0]),
                        summary::signed_degrees(right[1])
                    ),
                    cx,
                ),
                info_row("Eyes meet", summary::eyes_meet(left[0], right[0]), cx),
            ],
            None => vec![info_row("Gaze", "Not tracked separately", cx)],
        };
        let gaze = Panel::new("Gaze, seen from above")
            .child(GazeView {
                output,
                left_color,
                right_color,
            })
            .child(
                h_flex()
                    .gap_4()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(legend("Left eye", left_color))
                    .child(legend("Right eye", right_color)),
            )
            .child(div().text_xs().text_color(theme.muted_foreground).child(
                "Each line shows where one eye is looking. Look at something close and the lines should cross in front of you. Yaw and pitch are in degrees.",
            ))
            .child(v_flex().gap_2().children(gaze_rows));

        let cameras = Panel::new("Eye cameras")
            .child(SnapshotView { image })
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(match snapshot {
                        Some((sequence, age)) => format!("Snapshot {sequence} · {age} ms old"),
                        None => "To see the eye cameras here, set Eye-camera preview snapshots above 0 in the headset app.".into(),
                    }),
            );

        let sample = status.and_then(|status| status.eyes.sample.as_ref());
        let eyes = status.map(|status| &status.eyes);
        let setup = Panel::new("Setup").child(
            v_flex()
                .gap_2()
                .child(info_row(
                    "Engine profile",
                    sample
                        .map(|sample| sample.engine_profile.to_string())
                        .unwrap_or_else(|| "—".into()),
                    cx,
                ))
                .child(info_row(
                    "Per-eye model",
                    match sample {
                        Some(sample) if sample.model_patched => "Active",
                        Some(_) => "Not active",
                        None => "—",
                    },
                    cx,
                ))
                .child(info_row(
                    "Recentered",
                    match status {
                        Some(status) if status.settings.eye_offsets.is_some() => "Yes",
                        Some(_) => "No",
                        None => "—",
                    },
                    cx,
                ))
                .child(info_row(
                    "Calibration file",
                    eyes.and_then(|eyes| {
                        eyes.calibration
                            .rsplit(['\\', '/'])
                            .next()
                            .filter(|name| !name.is_empty())
                            .map(str::to_string)
                    })
                    .unwrap_or_else(|| "—".into()),
                    cx,
                ))
                .child(info_row(
                    "Depth accuracy",
                    match eyes {
                        Some(eyes) if eyes.convergence_calibrated => "Validated",
                        Some(eyes) if !eyes.calibration.is_empty() => "Not validated",
                        _ => "—",
                    },
                    cx,
                )),
        );

        let settings = status.map(|status| &status.settings);
        let online = status.is_some();
        let troubleshooting = Panel::new("Avatar's eyes move the wrong way?")
            .child(div().text_sm().text_color(theme.muted_foreground).child(
                "Looking left should turn both avatar eyes left, and looking at something close should turn both inward.",
            ))
            .child(
                Switch::new("eye-swap")
                    .label("Swap left and right eyes")
                    .checked(settings.is_some_and(|settings| settings.eye_swap_output))
                    .disabled(!online)
                    .on_change(cx.listener(|page, checked: &bool, _, cx| {
                        page.change("eye_swap_output", *checked, cx)
                    })),
            )
            .child(
                Switch::new("eye-invert")
                    .label("Mirror left and right")
                    .checked(settings.is_some_and(|settings| settings.eye_invert_yaw))
                    .disabled(!online)
                    .on_change(cx.listener(|page, checked: &bool, _, cx| {
                        page.change("eye_invert_yaw", *checked, cx)
                    })),
            );

        v_flex()
            .gap_6()
            .child(
                PageHeader::new(
                    "Eyes",
                    "Each eye's own gaze from the headset, so your avatar's eyes can converge on close things.",
                )
                .trailing(StatusPill::new(reading.tone, reading.value)),
            )
            .child(recenter)
            .child(
                div()
                    .grid()
                    .grid_cols(if two_columns { 2 } else { 1 })
                    .gap_4()
                    .child(gaze)
                    .child(cameras)
                    .child(setup)
                    .child(troubleshooting),
            )
    }
}

fn legend(label: &'static str, color: Hsla) -> impl IntoElement {
    h_flex()
        .gap_1p5()
        .child(div().w(px(12.)).h(px(3.)).rounded_full().bg(color))
        .child(label)
}

/// Both gaze rays drawn from above, to scale with a typical 63 mm eye
/// separation: the face at the bottom, looking up the view.
#[derive(IntoElement)]
struct GazeView {
    output: Option<[[f32; 2]; 2]>,
    left_color: Hsla,
    right_color: Hsla,
}

/// The drawing's reference width; distances scale with the actual width.
const GAZE_REFERENCE_WIDTH: f32 = 480.;
const GAZE_PIXELS_PER_METRE: f32 = 300.;
const HALF_EYE_SEPARATION_M: f32 = 0.0315;
const GAZE_DISTANCES_M: [f32; 2] = [0.25, 0.5];

impl RenderOnce for GazeView {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let (grid, faint) = (theme.border, theme.muted_foreground);
        let (left_color, right_color, output) = (self.left_color, self.right_color, self.output);
        // Distance labels sit at fixed fractions of the height, because the
        // drawing keeps a 2:1 shape at any width.
        let labels = GAZE_DISTANCES_M.map(|metres| {
            let unit_height = GAZE_REFERENCE_WIDTH / 2.;
            let y = (unit_height - 16. - metres * GAZE_PIXELS_PER_METRE) / unit_height;
            div()
                .absolute()
                .left(px(8.))
                .top(relative(y))
                .mt(px(-18.))
                .text_xs()
                .text_color(faint)
                .child(format!("{metres} m"))
        });
        div()
            .relative()
            .w_full()
            .aspect_ratio(2.)
            .rounded(theme.radius)
            .overflow_hidden()
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
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

                        for metres in GAZE_DISTANCES_M {
                            let y = base - metres * scale;
                            let mut line = PathBuilder::stroke(px(1.));
                            line.move_to(at(0., y));
                            line.line_to(at(width, y));
                            if let Ok(path) = line.build() {
                                window.paint_path(path, grid);
                            }
                        }
                        let mut centre = PathBuilder::stroke(px(1.)).dash_array(&[px(4.), px(6.)]);
                        centre.move_to(at(middle, base));
                        centre.line_to(at(middle, 0.));
                        if let Ok(path) = centre.build() {
                            window.paint_path(path, grid);
                        }

                        let eyes = [
                            (middle - HALF_EYE_SEPARATION_M * scale, left_color),
                            (middle + HALF_EYE_SEPARATION_M * scale, right_color),
                        ];
                        for (index, (x, color)) in eyes.into_iter().enumerate() {
                            if let Some(output) = output {
                                // Yaw is positive to the right, so a converging
                                // left eye turns right.
                                let yaw = output[index][0].to_radians();
                                let length = 600. * unit;
                                let mut ray = PathBuilder::stroke(px(2.));
                                ray.move_to(at(x, base));
                                ray.line_to(at(x + yaw.sin() * length, base - yaw.cos() * length));
                                if let Ok(path) = ray.build() {
                                    window.paint_path(path, color);
                                }
                            }
                            let dot = Bounds::centered_at(at(x, base), size(px(10.), px(10.)));
                            window.paint_quad(fill(dot, color).corner_radii(px(5.)));
                        }
                    },
                )
                .size_full(),
            )
            .children(labels)
    }
}

/// The latest eye camera snapshot, or a placeholder when there is none.
#[derive(IntoElement)]
struct SnapshotView {
    image: Option<Arc<RenderImage>>,
}

impl RenderOnce for SnapshotView {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let view = div()
            .relative()
            .w_full()
            .aspect_ratio(2.)
            .rounded(theme.radius)
            .overflow_hidden()
            .border_1()
            .border_color(theme.border);
        match self.image {
            // Camera snapshots are grayscale raster data on black.
            Some(image) => view
                .bg(black())
                .child(img(image).size_full().object_fit(ObjectFit::Contain)),
            None => view.bg(theme.group_box).child(
                v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .text_color(theme.muted_foreground)
                    .child(Icon::new(IconName::CameraOff).large())
                    .child(div().text_sm().child("No eye camera images")),
            ),
        }
    }
}
