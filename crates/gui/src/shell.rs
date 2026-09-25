//! The window: title bar, navigation on the left, and the current page.
use crate::camera::MouthPage;
use crate::daemon::{Camera, DaemonClient, DEFAULT_ADDRESS};
use crate::eyes::EyesPage;
use crate::headset::HeadsetPage;
use crate::home::{HomePage, OpenPage};
use crate::launcher::Launcher;
use crate::live::{CameraFeed, DaemonState};
use crate::summary::{Connection, Tone};
use crate::tongue::TongueTraining;
use crate::widgets::StatusPill;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::sidebar::{
    Sidebar, SidebarCollapsible, SidebarFooter, SidebarGroup, SidebarMenu, SidebarMenuItem,
};
use gpui_kit::component::{
    h_flex, v_flex, ActiveTheme as _, Icon, Sizable as _, StyledExt as _, Theme, ThemeMode,
    TitleBar,
};
use gpui_kit::{
    div, px, AnyView, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement, Render, StatefulInteractiveElement as _, Styled, Subscription, Window,
};
use std::sync::Arc;

const NAV_WIDTH: f32 = 220.;
const PAGE_PADDING: f32 = 24.;
const PAGE_MAX_WIDTH: f32 = 1280.;

/// Width a page's content gets: the window less the navigation and the page
/// padding, up to the page's maximum.
pub fn content_width(window: &Window) -> f32 {
    (window.viewport_size().width.as_f32() - NAV_WIDTH).min(PAGE_MAX_WIDTH) - 2. * PAGE_PADDING
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Home,
    Headset,
    Mouth,
    Eyes,
}

pub struct Workspace {
    page: Page,
    daemon: Entity<DaemonState>,
    mouth_feed: Entity<CameraFeed>,
    eye_feed: Entity<CameraFeed>,
    home: Entity<HomePage>,
    headset: Entity<HeadsetPage>,
    mouth: Entity<MouthPage>,
    tongue: Entity<TongueTraining>,
    eyes: Entity<EyesPage>,
    _subscriptions: [Subscription; 2],
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let client = Arc::new(DaemonClient::new(DEFAULT_ADDRESS));
        let daemon = cx.new(|cx| DaemonState::new(client.clone(), cx));
        let mouth_feed = cx.new(|cx| CameraFeed::new(client.clone(), Camera::Mouth, cx));
        let eye_feed = cx.new(|cx| CameraFeed::new(client, Camera::Eyes, cx));
        let launcher = cx.new(|cx| {
            let mut launcher = Launcher::new(daemon.clone());
            launcher.start_unless_running(cx);
            launcher
        });
        let home = cx.new(|cx| HomePage::new(daemon.clone(), launcher.clone(), cx));
        let headset = cx.new(|cx| HeadsetPage::new(daemon.clone(), window, cx));
        let tongue = cx.new(|cx| TongueTraining::new(daemon.clone(), window, cx));
        let mouth = cx.new(|cx| {
            MouthPage::new(
                daemon.clone(),
                mouth_feed.clone(),
                launcher.clone(),
                tongue.clone(),
                cx,
            )
        });
        let eyes =
            cx.new(|cx| EyesPage::new(daemon.clone(), eye_feed.clone(), launcher.clone(), cx));
        let subscriptions = [
            // The title bar shows whether the daemon is reachable.
            cx.observe(&daemon, |_, _, cx| cx.notify()),
            cx.subscribe(&home, |this, _, OpenPage(page), cx| this.open(*page, cx)),
        ];
        Self {
            page: Page::Home,
            daemon,
            mouth_feed,
            eye_feed,
            home,
            headset,
            mouth,
            tongue,
            eyes,
            _subscriptions: subscriptions,
        }
    }

    fn open(&mut self, page: Page, cx: &mut Context<Self>) {
        if self.page == page {
            return;
        }
        self.page = page;
        self.headset.update(cx, |headset, cx| {
            headset.set_watching(page == Page::Headset, cx)
        });
        self.mouth_feed
            .update(cx, |feed, cx| feed.set_watching(page == Page::Mouth, cx));
        self.tongue.update(cx, |tongue, cx| {
            tongue.set_watching(page == Page::Mouth, cx)
        });
        self.eye_feed
            .update(cx, |feed, cx| feed.set_watching(page == Page::Eyes, cx));
        // Both camera pages show values that move with the wearer.
        self.daemon.update(cx, |daemon, _| {
            daemon.set_fast(matches!(page, Page::Mouth | Page::Eyes))
        });
        cx.notify();
    }

    fn nav_item(
        &self,
        label: &'static str,
        icon: IconName,
        page: Page,
        cx: &mut Context<Self>,
    ) -> SidebarMenuItem {
        SidebarMenuItem::new(label)
            .icon(icon)
            .active(self.page == page)
            .on_click(cx.listener(move |this, _, _, cx| this.open(page, cx)))
    }

    fn title_bar(&self, cx: &mut Context<Self>) -> TitleBar {
        let (tone, label) = match self.daemon.read(cx).connection() {
            Connection::Connecting => (Tone::Waiting, "Connecting"),
            Connection::Online => (Tone::Good, "Connected"),
            Connection::Offline { .. } => (Tone::Problem, "VRFT not running"),
        };
        let dark = cx.theme().is_dark();
        TitleBar::new()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(20.))
                            .rounded(cx.theme().radius)
                            .bg(cx.theme().primary)
                            .text_color(cx.theme().primary_foreground)
                            .child(Icon::new(IconName::ScanFace).xsmall()),
                    )
                    .child(div().text_sm().font_semibold().child("VRFT")),
            )
            .child(
                h_flex()
                    .gap_2()
                    .pr_2()
                    .child(StatusPill::new(tone, label))
                    .child(
                        Button::new("theme-mode")
                            .ghost()
                            .xsmall()
                            .icon(if dark { IconName::Sun } else { IconName::Moon })
                            .tooltip(if dark {
                                "Use the light theme"
                            } else {
                                "Use the dark theme"
                            })
                            .on_click(move |_, window, cx| {
                                let mode = if dark {
                                    ThemeMode::Light
                                } else {
                                    ThemeMode::Dark
                                };
                                Theme::change(mode, Some(window), cx);
                            }),
                    ),
            )
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page: AnyView = match self.page {
            Page::Home => self.home.clone().into(),
            Page::Headset => self.headset.clone().into(),
            Page::Mouth => self.mouth.clone().into(),
            Page::Eyes => self.eyes.clone().into(),
        };
        let address = self.daemon.read(cx).address().to_string();
        let navigation = Sidebar::new("navigation")
            .collapsible(SidebarCollapsible::None)
            .w(px(NAV_WIDTH))
            .child(
                SidebarGroup::new("Overview").child(SidebarMenu::new().child(self.nav_item(
                    "Home",
                    IconName::LayoutDashboard,
                    Page::Home,
                    cx,
                ))),
            )
            .child(
                SidebarGroup::new("Quest Pro").child(
                    SidebarMenu::new()
                        .child(self.nav_item("Headset", IconName::Glasses, Page::Headset, cx))
                        .child(self.nav_item("Mouth", IconName::Camera, Page::Mouth, cx))
                        .child(self.nav_item("Eyes", IconName::ScanEye, Page::Eyes, cx)),
                ),
            )
            .footer(
                SidebarFooter::new().child(
                    v_flex()
                        .gap_0p5()
                        .text_xs()
                        .child(div().font_medium().child("vrft_d"))
                        .child(
                            div()
                                .text_color(cx.theme().sidebar_foreground.opacity(0.7))
                                .child(address),
                        ),
                ),
            );

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.title_bar(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .child(navigation)
                    .child(
                        div()
                            .id("page")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_y_scroll()
                            .child(
                                div()
                                    .w_full()
                                    .max_w(px(PAGE_MAX_WIDTH))
                                    .p(px(PAGE_PADDING))
                                    .child(page),
                            ),
                    ),
            )
    }
}
