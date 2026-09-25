//! Mouth: the live stereo image from the Quest Pro's lower-face cameras, the
//! tongue VRChat receives from it, and the tongue model behind it.
use crate::daemon::Status;
use crate::launcher::{LaunchState, Launcher, StartVrft};
use crate::live::{CameraFeed, DaemonState};
use crate::summary::{self, Connection, Rates, Tone, TongueReading, TongueState};
use crate::tongue::{DirectionPad, TongueTraining};
use crate::widgets::{info_row, Meter, PageHeader, Panel, StatusPill};
use gpui_kit::assets::IconName;
use gpui_kit::component::{v_flex, ActiveTheme as _, Icon, Sizable as _, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    black, div, img, px, AnyElement, AnyView, App, Context, Entity, IntoElement, ObjectFit,
    ParentElement, Render, RenderImage, RenderOnce, SharedString, Styled, StyledImage as _,
    Subscription, Window,
};
use std::sync::Arc;

/// Below this content width the side panel moves under the camera image.
const SIDE_BY_SIDE_WIDTH: f32 = 860.;
const PANEL_WIDTH: f32 = 300.;

pub struct MouthPage {
    daemon: Entity<DaemonState>,
    camera: Entity<CameraFeed>,
    launcher: Entity<Launcher>,
    training: Entity<TongueTraining>,
    _subscriptions: [Subscription; 3],
}

impl MouthPage {
    pub fn new(
        daemon: Entity<DaemonState>,
        camera: Entity<CameraFeed>,
        launcher: Entity<Launcher>,
        training: Entity<TongueTraining>,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = [
            cx.observe(&daemon, |_, _, cx| cx.notify()),
            cx.observe(&camera, |_, _, cx| cx.notify()),
            cx.observe(&launcher, |_, _, cx| cx.notify()),
        ];
        Self {
            daemon,
            camera,
            launcher,
            training,
            _subscriptions: subscriptions,
        }
    }
}

impl Render for MouthPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.daemon.read(cx);
        let status = state.status();
        let rates = state.rates();
        let starting = *self.launcher.read(cx).state() == LaunchState::Starting;
        let screen = screen_state(state.connection(), status, &rates, starting);
        let image = self.camera.read(cx).image();
        let start = matches!(screen, Screen::Offline)
            .then(|| StartVrft::new(self.launcher.clone()).into_any_element());
        let pill = match &screen {
            Screen::Live { fps } => StatusPill::new(
                Tone::Good,
                match fps {
                    Some(fps) => format!("Live · {fps:.0} fps"),
                    None => "Live".into(),
                },
            ),
            Screen::Waiting { .. } => StatusPill::new(Tone::Waiting, "Waiting"),
            Screen::Offline => StatusPill::new(Tone::Problem, "VRFT not running"),
        };
        let wide = crate::shell::content_width(window) >= SIDE_BY_SIDE_WIDTH;
        let panel = side_panel(status, &rates, cx);

        v_flex()
            .gap_6()
            .child(
                PageHeader::new(
                    "Mouth",
                    "What the headset's lower-face cameras see, the tongue your avatar gets, and the model that tracks it.",
                )
                .trailing(pill),
            )
            .child(
                div()
                    .flex()
                    .gap_4()
                    .map(|this| {
                        if wide {
                            this.flex_row().items_start()
                        } else {
                            this.flex_col()
                        }
                    })
                    .child(div().flex_1().min_w_0().child(CameraScreen {
                        image,
                        screen,
                        start,
                    }))
                    .child(
                        div()
                            .flex_none()
                            .when(wide, |this| this.w(px(PANEL_WIDTH)))
                            .child(panel),
                    ),
            )
            .child(AnyView::from(self.training.clone()))
    }
}

enum Screen {
    Live {
        fps: Option<f32>,
    },
    Waiting {
        title: SharedString,
        detail: SharedString,
    },
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
        (Connection::Online, Some(status)) => Screen::Waiting {
            title: "Waiting for the mouth cameras".into(),
            detail: status.status.clone().into(),
        },
        (Connection::Connecting, _) => Screen::Waiting {
            title: "Connecting to VRFT".into(),
            detail: SharedString::default(),
        },
        _ if starting => Screen::Waiting {
            title: "Starting VRFT".into(),
            detail: "Waiting for vrft_d to answer.".into(),
        },
        _ => Screen::Offline,
    }
}

/// The stereo camera image, or what is keeping it from showing.
#[derive(IntoElement)]
struct CameraScreen {
    image: Option<Arc<RenderImage>>,
    screen: Screen,
    /// The offer to start VRFT, shown while it isn't running.
    start: Option<AnyElement>,
}

impl RenderOnce for CameraScreen {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let screen = div()
            .relative()
            .w_full()
            .aspect_ratio(2.)
            .rounded(theme.radius_lg)
            .overflow_hidden()
            .border_1()
            .border_color(theme.border);
        let (icon, title, detail) = match self.screen {
            // The camera image is grayscale raster data; its letterbox stays
            // black in either theme.
            Screen::Live { .. } => {
                return screen.bg(black()).when_some(self.image, |this, image| {
                    this.child(img(image).size_full().object_fit(ObjectFit::Contain))
                });
            }
            Screen::Waiting { title, detail } => (IconName::CameraOff, title, detail),
            Screen::Offline => (
                IconName::Unplug,
                "VRFT isn't running".into(),
                "VRFT needs to be running to show the cameras.".into(),
            ),
        };
        // Without a live stream, say so in words rather than showing a stale
        // or empty picture.
        screen.bg(theme.group_box).child(
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_1()
                .p_6()
                .text_center()
                .child(
                    div()
                        .mb_2()
                        .text_color(theme.muted_foreground)
                        .child(Icon::new(icon).large()),
                )
                .child(
                    div()
                        .text_base()
                        .font_semibold()
                        .text_color(theme.foreground)
                        .child(title),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(detail),
                )
                .children(self.start.map(|start| div().mt_4().child(start))),
        )
    }
}

fn side_panel(status: Option<&Status>, rates: &Rates, cx: &App) -> AnyElement {
    let tongue = status
        .map(|status| summary::tongue(status, rates.tracking()))
        .unwrap_or(TongueReading {
            state: TongueState::NotTracked,
            out: 0.,
            horizontal: 0.,
            vertical: 0.,
        });
    let model = status.and_then(|status| status.model.as_ref());
    let rows: [(&str, String); 5] = [
        (
            "Frame rate",
            rates
                .camera_fps
                .filter(|_| status.is_some_and(summary::camera_live))
                .map(|fps| format!("{fps:.0} fps"))
                .unwrap_or_else(|| "—".into()),
        ),
        (
            "Frame age",
            status
                .and_then(|status| status.frame_age_ms)
                .map(|age| format!("{age} ms"))
                .unwrap_or_else(|| "—".into()),
        ),
        (
            "Model time",
            model
                .filter(|model| model.fresh)
                .map(|model| format!("{:.1} ms per frame", model.inference_ms))
                .unwrap_or_else(|| "—".into()),
        ),
        (
            "Skipped frames",
            model
                .map(|model| model.skipped_frames.to_string())
                .unwrap_or_else(|| "—".into()),
        ),
        (
            "VRChat tongue from",
            match status
                .and_then(|status| status.output.as_ref())
                .map(|output| output.source.as_str())
            {
                Some("enhanced model") => "Mouth cameras".into(),
                Some("tracking module") => "Tracking module".into(),
                Some(other) => other.to_string(),
                None => "—".into(),
            },
        ),
    ];
    v_flex()
        .gap_4()
        .child(
            Panel::new("Your tongue").child(
                v_flex()
                    .gap_4()
                    .items_center()
                    .child(tongue_pad(tongue))
                    .child(
                        v_flex()
                            .w_full()
                            .gap_2()
                            .child(info_row(
                                "Position",
                                match tongue.state {
                                    TongueState::NotTracked => "Not tracked",
                                    TongueState::In => "In",
                                    TongueState::Out => "Out",
                                },
                                cx,
                            ))
                            .child(info_row(
                                "How far out",
                                match tongue.state {
                                    TongueState::NotTracked => "—".to_string(),
                                    _ => format!("{:.0}%", tongue.out * 100.),
                                },
                                cx,
                            ))
                            .child(Meter::new(tongue.out, cx.theme().chart_2)),
                    ),
            ),
        )
        .child(
            Panel::new("Stream").child(
                v_flex().gap_2().children(
                    rows.into_iter()
                        .map(|(label, value)| info_row(label, value, cx)),
                ),
            ),
        )
        .into_any_element()
}

const PAD_SIZE: f32 = 200.;

/// Tongue direction as seen in a mirror: your right is on the right. The dot
/// grows the further out the tongue is.
fn tongue_pad(reading: TongueReading) -> DirectionPad {
    let pad = DirectionPad::new(PAD_SIZE);
    if reading.state == TongueState::Out {
        pad.dot(
            reading.horizontal,
            reading.vertical,
            14. + 14. * reading.out,
        )
    } else {
        pad
    }
}
