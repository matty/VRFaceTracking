//! Home: whether tracking works, and the state of each part of the pipeline.
use crate::launcher::{LaunchState, Launcher, StartVrft};
use crate::live::DaemonState;
use crate::shell::Page;
use crate::summary::{self, Connection, Reading, Tone};
use crate::widgets::{tone_color, PageHeader, Section, StatTile};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, Sizable as _, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, AnyElement, App, Context, Entity, EventEmitter, IntoElement, ParentElement, Render,
    Styled, Subscription, Window,
};

/// Asks the window to show another page.
pub struct OpenPage(pub Page);

pub struct HomePage {
    daemon: Entity<DaemonState>,
    launcher: Entity<Launcher>,
    _subscriptions: [Subscription; 2],
}

impl EventEmitter<OpenPage> for HomePage {}

/// Narrowest a tile gets before the grid drops a column.
const MIN_TILE_WIDTH: f32 = 210.;
const TILE_GAP: f32 = 12.;

impl HomePage {
    pub fn new(
        daemon: Entity<DaemonState>,
        launcher: Entity<Launcher>,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = [
            cx.observe(&daemon, |_, _, cx| cx.notify()),
            cx.observe(&launcher, |_, _, cx| cx.notify()),
        ];
        Self {
            daemon,
            launcher,
            _subscriptions: subscriptions,
        }
    }
}

impl Render for HomePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.daemon.read(cx);
        let status = state.status();
        let rates = state.rates();
        let online = *state.connection() == Connection::Online;
        let launch = self.launcher.read(cx).state().clone();
        let headline = match &launch {
            LaunchState::Starting if !online => Reading::new(
                Tone::Waiting,
                "Starting VRFT",
                format!("Waiting for vrft_d to answer on {}.", state.address()),
            ),
            _ => summary::headline(state.connection(), status, &rates),
        };
        // Offer to start the daemon once it's known not to be running, and
        // keep the offer up while it starts or if starting failed.
        let offer_start = !online
            && (matches!(state.connection(), Connection::Offline { .. })
                || launch != LaunchState::Idle);
        let mut engine = summary::engine(status, state.address());
        if let (true, LaunchState::Failed(message)) = (online, &launch) {
            engine = Reading::new(Tone::Problem, engine.value, message.to_string());
        }
        let stop = (online && status.is_some_and(|status| status.daemon.is_some())).then(|| {
            let launcher = self.launcher.clone();
            Button::new("stop-vrft")
                .ghost()
                .xsmall()
                .label("Stop")
                .loading(launch == LaunchState::Stopping)
                .on_click(move |_, _, cx| launcher.update(cx, |launcher, cx| launcher.stop(cx)))
        });
        let vrft = [
            StatTile::new(IconName::Cpu, "Engine", engine)
                .when_some(stop, |tile, stop| tile.action(stop)),
            StatTile::new(
                IconName::Puzzle,
                "Tracking module",
                summary::module(status, &rates),
            ),
            StatTile::new(
                IconName::Send,
                "OSC output",
                summary::output(status, &rates),
            ),
        ];
        let cameras = summary::mouth_cameras(status, &rates);
        let quest_pro = [
            StatTile::new(IconName::Glasses, "Headset", summary::headset(status)).action(
                Button::new("open-headset")
                    .ghost()
                    .xsmall()
                    .label("Manage")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(OpenPage(Page::Headset)))),
            ),
            StatTile::new(IconName::Camera, "Mouth cameras", cameras).action(
                Button::new("open-mouth-cameras")
                    .ghost()
                    .xsmall()
                    .label("View")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(OpenPage(Page::MouthCameras)))),
            ),
            StatTile::new(
                IconName::ScanFace,
                "Tongue model",
                summary::tongue_model(status),
            ),
            StatTile::new(IconName::ScanEye, "Eye gaze", summary::eyes(status)).action(
                Button::new("open-eyes")
                    .ghost()
                    .xsmall()
                    .label("View")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(OpenPage(Page::Eyes)))),
            ),
        ];
        let available = crate::shell::content_width(window);

        v_flex()
            .gap_6()
            .child(PageHeader::new(
                "Home",
                "Face tracking from your headset to VRChat, at a glance.",
            ))
            .child(Headline {
                reading: headline,
                action: offer_start
                    .then(|| StartVrft::new(self.launcher.clone()).into_any_element()),
            })
            .child(Section::new("VRFT").child(tile_grid(vrft, available)))
            .child(Section::new("Quest Pro").child(tile_grid(quest_pro, available)))
    }
}

fn tile_grid(tiles: impl IntoIterator<Item = StatTile>, available: f32) -> AnyElement {
    let tiles: Vec<StatTile> = tiles.into_iter().collect();
    let fit = ((available + TILE_GAP) / (MIN_TILE_WIDTH + TILE_GAP)).floor() as u16;
    let columns = fit.clamp(1, tiles.len().max(1) as u16);
    div()
        .grid()
        .grid_cols(columns)
        .gap(px(TILE_GAP))
        .children(tiles)
        .into_any_element()
}

/// The page's focal point: one sentence on whether tracking works, and what
/// to do if it doesn't.
#[derive(IntoElement)]
struct Headline {
    reading: Reading,
    action: Option<AnyElement>,
}

impl gpui_kit::RenderOnce for Headline {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = tone_color(self.reading.tone, cx);
        let icon = match self.reading.tone {
            Tone::Good => IconName::CircleCheck,
            Tone::Waiting => IconName::Activity,
            Tone::Problem => IconName::TriangleAlert,
            Tone::Off => IconName::Unplug,
        };
        let theme = cx.theme();
        h_flex()
            .items_start()
            .gap_4()
            .p_5()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(color.opacity(0.3))
            .bg(color.opacity(0.07))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(40.))
                    .rounded(theme.radius_lg)
                    .bg(color.opacity(0.15))
                    .text_color(color)
                    .child(Icon::new(icon)),
            )
            .child(
                v_flex()
                    .gap_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_lg()
                            .font_semibold()
                            .text_color(theme.foreground)
                            .child(self.reading.value),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(self.reading.detail),
                    )
                    .children(self.action.map(|action| div().mt_3().child(action))),
            )
    }
}
