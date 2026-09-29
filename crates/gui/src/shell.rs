//! The window: navigation down the left, from the top of the window, and
//! the current page beside it under a strip that moves the window and holds
//! its controls. Home, Modules, Tracking settings and Settings are the app's
//! own; every other
//! page comes from an extension.
use crate::extension_switches::ExtensionSwitches;
use crate::home::HomePage;
use crate::modules::ModulesPage;
use crate::settings::SettingsPage;
use crate::tracking::TrackingPage;
use crate::updates::{UpdateState, Updater};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    h_flex, v_flex, Icon, InteractiveElementExt as _, Sizable as _, StyledExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, linear_color_stop, linear_gradient, px, svg, AnyElement, AnyView, App, AppContext as _,
    Context, Entity, InteractiveElement as _, IntoElement, ParentElement, Render, ScrollHandle,
    SharedString, StatefulInteractiveElement as _, Styled, Subscription, Window, WindowControlArea,
};
use rust_i18n::t;
use std::collections::BTreeMap;
use std::sync::Arc;
use vrft_gui_core::client::{DaemonClient, DEFAULT_ADDRESS};
use vrft_gui_core::extension::{GuiExtension, GuiHost};
use vrft_gui_core::launcher::Launcher;
use vrft_gui_core::live::DaemonState;
use vrft_gui_core::nav::{self, Navigation, PageId};
use vrft_gui_core::palette;
use vrft_gui_core::summary::{self, Tone};
use vrft_gui_core::widgets::{cap, StatusDot};
use vrft_gui_core::{NAV_WIDTH, PAGE_MAX_WIDTH, PAGE_PADDING};

/// The navigation's width with only its icons showing.
const NAV_COLLAPSED_WIDTH: f32 = 60.;
/// The strip above each page that moves the window.
const STRIP_HEIGHT: f32 = 40.;

/// The extensions built into this app, and which the daemon runs.
pub struct Extensions {
    pub list: Vec<Box<dyn GuiExtension>>,
    /// `extensions.<id>.enabled` from `config.json`, for while the daemon
    /// can't say.
    configured: BTreeMap<String, bool>,
}

impl Extensions {
    /// Whether extension `id` runs in the daemon, so its pages have
    /// something to show. The daemon knows; while it isn't running,
    /// `config.json` says whether it will.
    pub fn shown(&self, id: &str, daemon: &DaemonState) -> bool {
        match daemon.status().and_then(|status| status.daemon.as_ref()) {
            Some(report) => report
                .extension(id)
                .is_some_and(|extension| extension.enabled),
            None => self.configured(id),
        }
    }

    /// Whether extension `id` is on but didn't start, so its pages can only
    /// say why.
    pub fn failed(&self, id: &str, daemon: &DaemonState) -> bool {
        daemon
            .status()
            .and_then(|status| status.daemon.as_ref())
            .and_then(|report| report.extension(id))
            .is_some_and(|extension| extension.error.is_some())
    }

    /// Whether `config.json` turns extension `id` on, as it's on unless
    /// turned off.
    pub fn configured(&self, id: &str) -> bool {
        self.configured.get(id).copied().unwrap_or(true)
    }

    pub fn set_configured(&mut self, id: &str, enabled: bool) {
        self.configured.insert(id.to_string(), enabled);
    }
}

/// `extensions.<id>.enabled` for each extension `config.json` mentions.
fn read_configured() -> BTreeMap<String, bool> {
    let Some(path) = vrft_gui_core::paths::config_file() else {
        return BTreeMap::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    let Ok(config) = serde_json::from_str::<serde_json::Value>(&text) else {
        return BTreeMap::new();
    };
    config["extensions"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(id, block)| {
            let enabled = block["enabled"].as_bool().unwrap_or(true);
            (id.clone(), enabled)
        })
        .collect()
}

pub struct Workspace {
    navigation: Entity<Navigation>,
    daemon: Entity<DaemonState>,
    launcher: Entity<Launcher>,
    extensions: Entity<Extensions>,
    home: Entity<HomePage>,
    settings: Entity<SettingsPage>,
    modules: Entity<ModulesPage>,
    tracking: Entity<TrackingPage>,
    updater: Entity<Updater>,
    /// The navigation shows only its icons.
    nav_collapsed: bool,
    /// For capturing pages in development: `VRFT_GUI_SCROLL=<pixels>` holds
    /// the page scrolled that far. Debug builds only.
    dev_scroll: Option<(ScrollHandle, f32)>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let navigation = nav::install(cx);
        let client = Arc::new(DaemonClient::new(DEFAULT_ADDRESS));
        let daemon = cx.new(|cx| DaemonState::new(client, cx));
        let launcher = cx.new(|cx| {
            let mut launcher = Launcher::new(daemon.clone(), cx);
            // For capturing pages in development: `VRFT_GUI_STOPPED=1` opens
            // as though tracking was stopped. Debug builds only.
            if cfg!(debug_assertions) && std::env::var_os("VRFT_GUI_STOPPED").is_some() {
                launcher.hold_stopped();
            } else {
                launcher.start_unless_running(cx);
            }
            launcher
        });
        let host = GuiHost {
            daemon: daemon.clone(),
            launcher: launcher.clone(),
        };
        let list = crate::extensions::built_in()
            .into_iter()
            .map(|create| create(&host, window, cx))
            .collect();
        let extensions = cx.new(|_| Extensions {
            list,
            configured: read_configured(),
        });
        let home =
            cx.new(|cx| HomePage::new(daemon.clone(), launcher.clone(), extensions.clone(), cx));
        let updater = cx.new(Updater::new);
        let settings = cx.new(|cx| {
            SettingsPage::new(
                daemon.clone(),
                launcher.clone(),
                updater.clone(),
                window,
                cx,
            )
        });
        let switches = cx.new(|cx| {
            ExtensionSwitches::new(daemon.clone(), launcher.clone(), extensions.clone(), cx)
        });
        let modules =
            cx.new(|cx| ModulesPage::new(daemon.clone(), launcher.clone(), switches, window, cx));
        let tracking = cx.new(|cx| TrackingPage::new(daemon.clone(), launcher.clone(), window, cx));
        let subscriptions = vec![
            // The navigation shows how things stand, and an extension the
            // daemon stops running takes its pages with it.
            cx.observe(&daemon, |this, _, cx| {
                this.leave_hidden_page(cx);
                cx.notify();
            }),
            cx.observe(&launcher, |_, _, cx| cx.notify()),
            // The navigation says when an update is ready.
            cx.observe(&updater, |_, _, cx| cx.notify()),
            cx.observe(&extensions, |this, _, cx| {
                this.leave_hidden_page(cx);
                cx.notify();
            }),
            cx.observe(&navigation, |this, navigation, cx| {
                let page = navigation.read(cx).current();
                this.show(page, cx);
            }),
        ];
        // For capturing pages in development: `VRFT_GUI_PAGE=<page id>`
        // opens on that page. Debug builds only.
        if cfg!(debug_assertions) {
            if let Ok(id) = std::env::var("VRFT_GUI_PAGE") {
                let extensions = extensions.read(cx);
                let page = [
                    PageId::HOME,
                    PageId::MODULES,
                    PageId::TRACKING,
                    PageId::SETTINGS,
                ]
                .into_iter()
                .chain(
                    extensions
                        .list
                        .iter()
                        .flat_map(|extension| extension.pages().iter().map(|entry| entry.page)),
                )
                .find(|page| page.id == id);
                if let Some(page) = page {
                    navigation.update(cx, |navigation, cx| navigation.open(page, cx));
                }
            }
        }
        Self {
            navigation,
            daemon,
            launcher,
            extensions,
            home,
            settings,
            modules,
            tracking,
            updater,
            nav_collapsed: false,
            dev_scroll: std::env::var("VRFT_GUI_SCROLL")
                .ok()
                .filter(|_| cfg!(debug_assertions))
                .and_then(|pixels| pixels.parse().ok())
                .map(|pixels| (ScrollHandle::new(), pixels)),
            _subscriptions: subscriptions,
        }
    }

    /// Tells extensions which page shows, so they only work while theirs do.
    fn show(&mut self, page: PageId, cx: &mut Context<Self>) {
        let fast = self.extensions.update(cx, |extensions, cx| {
            let mut fast = false;
            for extension in &mut extensions.list {
                extension.page_changed(page, cx);
                fast |= extension.wants_fast_status(page);
            }
            fast
        });
        self.daemon.update(cx, |daemon, _| daemon.set_fast(fast));
        self.settings.update(cx, |settings, cx| {
            settings.set_watching(page == PageId::SETTINGS, cx)
        });
        self.modules.update(cx, |modules, cx| {
            modules.set_watching(page == PageId::MODULES, cx)
        });
        self.tracking.update(cx, |tracking, cx| {
            tracking.set_watching(page == PageId::TRACKING, cx)
        });
        cx.notify();
    }

    /// Goes Home if the page on show belongs to an extension that isn't.
    fn leave_hidden_page(&mut self, cx: &mut Context<Self>) {
        let current = self.navigation.read(cx).current();
        if current == PageId::HOME
            || current == PageId::SETTINGS
            || current == PageId::MODULES
            || current == PageId::TRACKING
            || self.page_view(current, cx).is_some()
        {
            return;
        }
        self.navigation
            .update(cx, |navigation, cx| navigation.open(PageId::HOME, cx));
    }

    /// `page`'s view, while its extension is shown.
    fn page_view(&self, page: PageId, cx: &App) -> Option<AnyView> {
        let daemon = self.daemon.read(cx);
        let extensions = self.extensions.read(cx);
        extensions
            .list
            .iter()
            .filter(|extension| extensions.shown(extension.id(), daemon))
            .flat_map(|extension| extension.pages())
            .find(|entry| entry.page == page)
            .map(|entry| entry.view.clone())
    }

    /// How things stand overall, as a mark beside Home: the same reading as
    /// Home's headline. Nothing when VRFT was stopped on purpose.
    fn home_tone(&self, cx: &App) -> Option<Tone> {
        let state = self.daemon.read(cx);
        let activity = self.launcher.read(cx).activity();
        let headline = summary::headline(
            state.connection(),
            state.status(),
            &state.rates(),
            &activity,
        );
        Some(headline.tone).filter(|tone| *tone != Tone::Off)
    }

    fn nav_item(
        &self,
        icon: IconName,
        page: PageId,
        tone: Option<Tone>,
        current: PageId,
    ) -> AnyElement {
        let active = current == page;
        let collapsed = self.nav_collapsed;
        // Only a part that's doing something, or needs attention, gets a mark.
        let mark = tone.filter(|tone| *tone != Tone::Off);
        h_flex()
            .id(SharedString::from(format!("nav-{}", page.id)))
            .h(px(34.))
            .px(px(10.))
            .gap(px(11.))
            .rounded(px(8.))
            .cursor_pointer()
            .text_size(px(13.5))
            .text_color(if active {
                palette::text()
            } else {
                gpui_kit::rgb(0xa8a8a8).into()
            })
            .when(active, |item| item.bg(gpui_kit::rgb(0x1f1f23)))
            .when(!active, |item| {
                item.hover(|style| {
                    style
                        .bg(gpui_kit::rgb(0x17171a))
                        .text_color(palette::text())
                })
            })
            .on_click(move |_, _, cx| nav::open_page(page, cx))
            .child(Icon::new(icon).size(px(16.)))
            .when(collapsed, |item| {
                item.justify_center()
                    .tooltip(move |window, cx| Tooltip::new(page.label()).build(window, cx))
            })
            .when(!collapsed, |item| {
                item.child(div().flex_1().min_w_0().truncate().child(page.label()))
                    .children(mark.map(StatusDot::new))
            })
            .into_any_element()
    }

    /// The navigation's top: VRFT's mark and name, which move the window,
    /// and the button that folds the navigation to its icons. The button sits
    /// beside the part that moves the window, never inside it: Windows treats
    /// any point in a drag area as the title bar, so a button within one
    /// can't be clicked.
    fn brand(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let collapsed = self.nav_collapsed;
        let logo = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(26.))
            .rounded(px(8.))
            // The app icon's own tile: its blue, bright at the top and deep at
            // the bottom, with the face in white.
            .bg(linear_gradient(
                180.,
                linear_color_stop(gpui_kit::rgb(0x1d4ed8), 0.),
                linear_color_stop(gpui_kit::rgb(0x0b1a4a), 1.),
            ))
            .child(
                svg()
                    .path(crate::assets::MARK)
                    .size(px(17.))
                    .text_color(palette::text()),
            );
        let toggle = Button::new("toggle-navigation")
            .ghost()
            .xsmall()
            .icon(if collapsed {
                IconName::PanelLeftOpen
            } else {
                IconName::PanelLeftClose
            })
            .tooltip(if collapsed {
                t!("shell.show_navigation")
            } else {
                t!("shell.show_only_icons")
            })
            .on_click(cx.listener(|this, _, _, cx| {
                this.nav_collapsed = !this.nav_collapsed;
                cx.notify();
            }));
        if collapsed {
            v_flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .id("brand-drag")
                        .pt_1()
                        .window_control_area(WindowControlArea::Drag)
                        .child(logo),
                )
                .child(toggle)
                .into_any_element()
        } else {
            h_flex()
                .h(px(36.))
                .pr_1()
                .child(
                    h_flex()
                        .id("brand-drag")
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .gap(px(10.))
                        .pl(px(6.))
                        .when(cfg!(target_os = "macos"), |row| row.pl(px(72.)))
                        .window_control_area(WindowControlArea::Drag)
                        .child(logo)
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(px(14.5))
                                .font_semibold()
                                .child(crate::updates::app_name()),
                        ),
                )
                .child(toggle)
                .into_any_element()
        }
    }

    fn navigation(&self, current: PageId, cx: &mut Context<Self>) -> impl IntoElement {
        let collapsed = self.nav_collapsed;
        let home_tone = self.home_tone(cx);
        let groups: Vec<AnyElement> = {
            let daemon = self.daemon.read(cx);
            let extensions = self.extensions.read(cx);
            extensions
                .list
                .iter()
                .filter(|extension| extensions.shown(extension.id(), daemon))
                .map(|extension| {
                    let failed = extensions.failed(extension.id(), daemon);
                    let items: Vec<AnyElement> = extension
                        .pages()
                        .iter()
                        .map(|entry| {
                            let tone = extension.nav_tone(entry.page, cx);
                            self.nav_item(entry.icon, entry.page, tone, current)
                        })
                        .collect();
                    let label = if collapsed {
                        div()
                            .mx_2()
                            .my(px(14.))
                            .h(px(1.))
                            .bg(palette::line_soft())
                            .into_any_element()
                    } else {
                        h_flex()
                            .gap_2()
                            .mx(px(10.))
                            .mt(px(22.))
                            .mb_2()
                            .child(cap(extension.name()))
                            .when(failed, |row| {
                                row.child(StatusDot::new(Tone::Problem)).child(
                                    div()
                                        .text_xs()
                                        .text_color(palette::signal_text())
                                        .child(t!("shell.extension_failed")),
                                )
                            })
                            .into_any_element()
                    };
                    v_flex()
                        .child(label)
                        .child(v_flex().gap_0p5().children(items))
                        .into_any_element()
                })
                .collect()
        };
        let version = self
            .daemon
            .read(cx)
            .status()
            .and_then(|status| status.daemon.as_ref())
            .map(|daemon| daemon.version.clone())
            .filter(|version| !version.is_empty())
            .unwrap_or_else(|| crate::updates::VERSION.to_string());
        let update_ready = matches!(self.updater.read(cx).state(), UpdateState::Ready(_));
        v_flex()
            .flex_none()
            .h_full()
            .w(px(if collapsed {
                NAV_COLLAPSED_WIDTH
            } else {
                NAV_WIDTH
            }))
            .bg(palette::rail())
            .border_r_1()
            .border_color(gpui_kit::rgb(0x1e1e22))
            .px(px(if collapsed { 10. } else { 12. }))
            .pt(px(10.))
            .pb(px(14.))
            .child(self.brand(cx))
            .child(
                v_flex()
                    .mt(px(18.))
                    .gap_0p5()
                    .child(self.nav_item(IconName::House, PageId::HOME, home_tone, current))
                    .child(self.nav_item(IconName::Package, PageId::MODULES, None, current))
                    .child(self.nav_item(
                        IconName::SlidersHorizontal,
                        PageId::TRACKING,
                        None,
                        current,
                    )),
            )
            .children(groups)
            .child(div().flex_1())
            .child(v_flex().gap_0p5().child(self.nav_item(
                IconName::Settings,
                PageId::SETTINGS,
                None,
                current,
            )))
            .when(update_ready && !collapsed, |nav| {
                nav.child(
                    h_flex()
                        .id("update-ready")
                        .mt_2()
                        .mx(px(10.))
                        .gap_1p5()
                        .cursor_pointer()
                        .text_size(px(12.))
                        .text_color(palette::text_2())
                        .hover(|style| style.text_color(palette::text()))
                        .on_click(|_, _, cx| nav::open_page(PageId::SETTINGS, cx))
                        .child(Icon::new(IconName::ArrowDown).size(px(13.)))
                        .child(t!("updates.sidebar_ready")),
                )
            })
            .when(!collapsed, |nav| {
                nav.child(
                    div()
                        .pt_3()
                        .px(px(10.))
                        .text_size(px(11.))
                        .font_family(palette::MONO_FONT)
                        .text_color(palette::text_3())
                        .child(t!("shell.version", version = version)),
                )
            })
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.navigation.read(cx).current();
        let page: AnyView = if current == PageId::SETTINGS {
            self.settings.clone().into()
        } else if current == PageId::MODULES {
            self.modules.clone().into()
        } else if current == PageId::TRACKING {
            self.tracking.clone().into()
        } else {
            self.page_view(current, cx)
                .unwrap_or_else(|| self.home.clone().into())
        };

        h_flex()
            .size_full()
            .items_stretch()
            .bg(palette::page())
            .text_color(palette::text())
            .child(self.navigation(current, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(WindowStrip)
                    .child(
                        // Each page keeps its own scroll position, so opening
                        // one starts where it was left rather than where the
                        // last page was.
                        div()
                            .id(SharedString::from(format!("page-{}", current.id)))
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .when_some(self.dev_scroll.as_ref(), |page, (handle, pixels)| {
                                handle.set_offset(gpui_kit::point(px(0.), px(-pixels)));
                                page.track_scroll(handle)
                            })
                            .child(
                                div()
                                    .w_full()
                                    .max_w(px(PAGE_MAX_WIDTH))
                                    .mx_auto()
                                    .px(px(PAGE_PADDING))
                                    .pt(px(6.))
                                    .pb(px(40.))
                                    .child(page),
                            ),
                    ),
            )
    }
}

/// The strip across the top of the page: it moves the window, and holds the
/// window's own buttons at its right.
#[derive(IntoElement)]
struct WindowStrip;

impl gpui_kit::RenderOnce for WindowStrip {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        h_flex()
            .flex_none()
            .w_full()
            .h(px(STRIP_HEIGHT))
            .child(
                div()
                    .id("window-drag")
                    .flex_1()
                    .h_full()
                    .window_control_area(WindowControlArea::Drag)
                    .when(!cfg!(target_os = "windows"), |strip| {
                        strip
                            .on_mouse_down(gpui_kit::MouseButton::Left, |_, window, _| {
                                window.start_window_move()
                            })
                            .on_double_click(|_, window, _| window.zoom_window())
                    }),
            )
            .when(!cfg!(target_os = "macos"), |strip| {
                strip
                    .child(CaptionButton::Minimize)
                    .child(if window.is_maximized() {
                        CaptionButton::Restore
                    } else {
                        CaptionButton::Maximize
                    })
                    .child(CaptionButton::Close)
            })
    }
}

/// One of the window's own buttons. On Windows the system handles the
/// click, snap layouts included; elsewhere the click does it.
#[derive(IntoElement, Clone, Copy)]
enum CaptionButton {
    Minimize,
    Maximize,
    Restore,
    Close,
}

impl gpui_kit::RenderOnce for CaptionButton {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let (id, icon, area) = match self {
            Self::Minimize => ("minimize", IconName::WindowMinimize, WindowControlArea::Min),
            Self::Maximize => ("maximize", IconName::WindowMaximize, WindowControlArea::Max),
            Self::Restore => ("restore", IconName::WindowRestore, WindowControlArea::Max),
            Self::Close => ("close", IconName::WindowClose, WindowControlArea::Close),
        };
        let close = matches!(self, Self::Close);
        div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(px(46.))
            .h_full()
            .text_color(palette::text_2())
            .hover(move |style| {
                if close {
                    // Windows' own close red, not the signal colour.
                    style
                        .bg(gpui_kit::rgb(0xc42b1c))
                        .text_color(palette::text())
                } else {
                    style
                        .bg(gpui_kit::rgb(0x1a1a1d))
                        .text_color(palette::text())
                }
            })
            .window_control_area(area)
            .when(!cfg!(target_os = "windows"), |button| {
                button.on_click(move |_, window, _| match self {
                    Self::Minimize => window.minimize_window(),
                    Self::Maximize | Self::Restore => window.zoom_window(),
                    Self::Close => window.remove_window(),
                })
            })
            .child(Icon::new(icon).small())
    }
}
