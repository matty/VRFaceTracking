//! First-launch setup, a guide in steps. On a new install it fills the
//! window: a rail down the left lists the steps, ticking off those done,
//! and the step on show sits beside it. It welcomes, has the tracking module
//! chosen, one already on this PC (such as the native modules that come with
//! VRFT) or one from the VRCFT module registry, asks yes or no for each
//! add-on for hardware only some people have, such as the Quest Pro, and
//! ends on a summary. Nothing is chosen for you: no module loads until one
//! is picked here or on the Modules page. Settings opens it again.
use crate::config_file;
use crate::modules::{icon_tile, matches, module_tile, runtime_label, unusable};
use crate::shell::{mark_tile, Extensions, WindowStrip};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{h_flex, v_flex, Disableable as _, Icon, Sizable as _, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, AnyElement, AppContext as _, Context, Div, ElementId, Entity, InteractiveElement as _,
    IntoElement, ParentElement, Render, SharedString, Stateful, StatefulInteractiveElement as _,
    Styled, Subscription, Task, Window, WindowControlArea,
};
use rust_i18n::t;
use std::time::Duration;
use vrft_gui_core::client::{
    ConfigPatch, DaemonClient, InstalledModule, ModuleOperation, Modules, OperationState,
    RegistryModule,
};
use vrft_gui_core::extension::SetupOffer;
use vrft_gui_core::launcher::{Launcher, StartVrft};
use vrft_gui_core::live::DaemonState;
use vrft_gui_core::nav::{self, PageId};
use vrft_gui_core::palette::{self, MONO_FONT};
use vrft_gui_core::summary::{Connection, Tone};
use vrft_gui_core::widgets::{cap, card, hint, mono, Notice, Section};
use vrft_gui_core::PAGE_PADDING;

/// Setup, which fills the window in place of the navigation and pages.
pub const PAGE: PageId = PageId::translated("setup", || t!("setup.page"));

/// The steps' rail.
const RAIL_WIDTH: f32 = 252.;
/// Widest a step's content gets: it reads top to bottom.
const CONTENT_WIDTH: f32 = 720.;

/// How often setup reads `/modules` while it shows the modules.
const POLL: Duration = Duration::from_secs(2);
/// How often while something is downloading or installing.
const BUSY_POLL: Duration = Duration::from_millis(300);

/// Modules setup doesn't offer: the test logger tracks nothing, and only
/// development builds have it.
const HIDDEN_MODULES: [&str; 1] = ["test_logger.dll"];

/// Tallest the registry's list gets before it scrolls.
const REGISTRY_HEIGHT: f32 = 340.;
/// A module's icon tile.
const TILE: f32 = 32.;
/// A card's corner inside its 1px edge, for a row that fills it.
const INNER_RADIUS: f32 = 13.;

/// The first two steps; the add-ons' follow, then the summary.
const WELCOME: usize = 0;
const MODULE: usize = 1;

/// An extension setup asks about.
struct Offer {
    id: &'static str,
    name: &'static str,
    icon: IconName,
    offer: SetupOffer,
}

/// How a step on the rail stands.
#[derive(Clone, Copy, PartialEq)]
enum Progress {
    Done,
    Current,
    Upcoming,
}

pub struct SetupPage {
    daemon: Entity<DaemonState>,
    launcher: Entity<Launcher>,
    extensions: Entity<Extensions>,
    offers: Vec<Offer>,
    /// [`WELCOME`], [`MODULE`], then one step per offer, then the summary.
    step: usize,
    modules: Option<Modules>,
    search: Entity<InputState>,
    /// The installed module chosen, by its key.
    choice: Option<String>,
    /// A registry module being installed, chosen once it is.
    awaiting: Option<String>,
    /// Each offer's answer, once given.
    answers: Vec<Option<bool>>,
    /// Waiting on VRFT.
    pending: bool,
    /// Why the last request failed.
    message: Option<Notice>,
    watching: bool,
    _poll: Task<()>,
    _request: Option<Task<()>>,
    _subscriptions: [Subscription; 3],
}

/// Whether `modules` has something running that setup should follow
/// closely.
fn busy(modules: Option<&Modules>) -> bool {
    modules.is_some_and(|modules| {
        modules.registry.loading
            || modules
                .operations
                .iter()
                .any(|operation| operation.state.running())
    })
}

/// Whether setup offers `module`.
fn offered(module: &InstalledModule) -> bool {
    !HIDDEN_MODULES
        .iter()
        .any(|hidden| module.file.eq_ignore_ascii_case(hidden))
}

/// The mark at the end of a row that can be chosen: a ring, filled with a
/// tick once chosen.
fn radio(chosen: bool) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(18.))
        .rounded_full()
        .border_1()
        .border_color(if chosen {
            palette::text()
        } else {
            palette::line_strong()
        })
        .when(chosen, |mark| {
            mark.bg(palette::text())
                .text_color(palette::page())
                .child(Icon::new(IconName::Check).size(px(12.)))
        })
}

/// A row in a card that can be chosen, `index` of `count`. Rows fill the
/// card, so the first and last take its corners.
fn choice_row(id: impl Into<ElementId>, index: usize, count: usize, chosen: bool) -> Stateful<Div> {
    h_flex()
        .id(id)
        .gap(px(14.))
        .px_4()
        .py(px(12.))
        .when(index > 0, |row| {
            row.border_t_1().border_color(palette::line_soft())
        })
        .when(index == 0, |row| row.rounded_t(px(INNER_RADIUS)))
        .when(index + 1 == count, |row| row.rounded_b(px(INNER_RADIUS)))
        .when(chosen, |row| row.bg(palette::inset()))
}

/// Download progress, or that it's installing, in a few characters.
fn progress(operation: &ModuleOperation) -> Div {
    let label = match (operation.state, operation.fraction) {
        (OperationState::Downloading, Some(fraction)) => t!(
            "modules.downloading_percent",
            percent = format!("{:.0}", fraction * 100.)
        ),
        (OperationState::Downloading, None) => t!("modules.downloading"),
        _ => t!("modules.installing"),
    };
    div()
        .flex()
        .flex_none()
        .h_7()
        .items_center()
        .font_family(MONO_FONT)
        .text_size(px(11.))
        .text_color(palette::text_2())
        .child(label)
}

/// A step's number on the rail: ticked once done, ringed brightly while
/// it's the step on show.
fn step_ring(number: usize, progress: Progress) -> Div {
    let ring = div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(26.))
        .rounded_full()
        .text_size(px(12.))
        .font_semibold();
    match progress {
        Progress::Done => ring
            .bg(palette::text())
            .text_color(palette::page())
            .child(Icon::new(IconName::Check).size(px(14.))),
        Progress::Current => ring
            .border_2()
            .border_color(palette::text())
            .text_color(palette::text())
            .child(number.to_string()),
        Progress::Upcoming => ring
            .border_1()
            .border_color(palette::line_strong())
            .text_color(palette::text_3())
            .child(number.to_string()),
    }
}

/// A step's title, words under it and what it asks, at the top of the step.
fn step_header(
    number: usize,
    total: usize,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
) -> Div {
    v_flex()
        .gap_2()
        .child(
            div()
                .font_family(MONO_FONT)
                .text_size(px(11.))
                .text_color(palette::text_3())
                .child(t!("setup.step", step = number, total = total)),
        )
        .child(
            div()
                .text_size(px(26.))
                .line_height(px(32.))
                .font_semibold()
                .child(title.into()),
        )
        .child(
            div()
                .max_w(px(600.))
                .text_sm()
                .text_color(palette::text_2())
                .child(description.into()),
        )
}

/// A round badge holding `icon`, above a step's title.
fn hero_badge(
    icon: IconName,
    color: gpui_kit::Hsla,
    bg: gpui_kit::Hsla,
    line: gpui_kit::Hsla,
) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(56.))
        .rounded_full()
        .bg(bg)
        .border_1()
        .border_color(line)
        .text_color(color)
        .child(Icon::new(icon).size(px(26.)))
}

/// One line of a summary: what, then how it's set.
fn summary_row(index: usize, label: SharedString, value: SharedString, on: bool) -> Div {
    h_flex()
        .gap_4()
        .px_5()
        .py(px(14.))
        .when(index > 0, |row| {
            row.border_t_1().border_color(palette::line_soft())
        })
        .child(
            div()
                .w(px(180.))
                .flex_none()
                .text_size(px(13.))
                .text_color(palette::text_3())
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(13.5))
                .font_medium()
                .text_color(if on {
                    palette::text()
                } else {
                    palette::text_2()
                })
                .child(value),
        )
}

impl SetupPage {
    pub fn new(
        daemon: Entity<DaemonState>,
        launcher: Entity<Launcher>,
        extensions: Entity<Extensions>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let offers: Vec<Offer> = extensions
            .read(cx)
            .list
            .iter()
            .filter_map(|extension| {
                extension.setup_offer().map(|offer| Offer {
                    id: extension.id(),
                    name: extension.name(),
                    icon: extension.icon(),
                    offer,
                })
            })
            .collect();
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("modules.search_placeholder")));
        let client = daemon.read(cx).client();
        let poll = cx.spawn_in(window, async move |this, cx| loop {
            let Ok((read, fast)) = this.update(cx, |page, cx| {
                let online = *page.daemon.read(cx).connection() == Connection::Online;
                (
                    page.watching && page.step == MODULE && online,
                    busy(page.modules.as_ref()),
                )
            }) else {
                break;
            };
            if read {
                let client = client.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move { client.modules() })
                    .await;
                let applied = this.update(cx, |page, cx| {
                    match result {
                        Ok(modules) => page.apply_modules(modules),
                        Err(error) => {
                            if page.modules.is_none() {
                                page.message =
                                    Some(Notice::error(&t!("modules.read_failed"), &error));
                            }
                        }
                    }
                    cx.notify();
                });
                if applied.is_err() {
                    break;
                }
            }
            cx.background_executor()
                .timer(if fast { BUSY_POLL } else { POLL })
                .await;
        });
        let subscriptions = [
            cx.observe(&daemon, |_, _, cx| cx.notify()),
            cx.observe(&launcher, |_, _, cx| cx.notify()),
            cx.observe(&search, |_, _, cx| cx.notify()),
        ];
        let answers = vec![None; offers.len()];
        Self {
            daemon,
            launcher,
            extensions,
            offers,
            step: WELCOME,
            modules: None,
            search,
            choice: None,
            awaiting: None,
            answers,
            pending: false,
            message: None,
            watching: false,
            _poll: poll,
            _request: None,
            _subscriptions: subscriptions,
        }
    }

    /// Setup shows, or stops showing. Each time it opens it starts again.
    pub fn set_watching(&mut self, watching: bool, cx: &mut Context<Self>) {
        if watching && !self.watching {
            self.step = WELCOME;
            self.choice = None;
            self.awaiting = None;
            self.answers = vec![None; self.offers.len()];
            self.message = None;
        }
        self.watching = watching;
        cx.notify();
    }

    /// The summary, the last step.
    fn done_step(&self) -> usize {
        MODULE + self.offers.len() + 1
    }

    /// The offer step `step` asks about, if it asks about one.
    fn offer_index(&self, step: usize) -> Option<usize> {
        step.checked_sub(MODULE + 1)
            .filter(|index| *index < self.offers.len())
    }

    /// Takes in what VRFT says about the modules: setup opened again starts
    /// from the module in use, and a module installed here is chosen once
    /// it's ready.
    fn apply_modules(&mut self, modules: Modules) {
        let usable = |module: &&InstalledModule| unusable(module, &modules).is_none();
        if self.choice.is_none() && self.awaiting.is_none() {
            self.choice = modules
                .installed
                .iter()
                .filter(|module| offered(module))
                .filter(usable)
                .find(|module| !modules.active.is_empty() && module.file == modules.active)
                .map(|module| module.file.clone());
        }
        if let Some(id) = &self.awaiting {
            let operation = modules
                .operations
                .iter()
                .find(|operation| &operation.module_id == id);
            match operation.map(|operation| operation.state) {
                Some(OperationState::Failed) => self.awaiting = None,
                Some(state) if state.running() => {}
                _ => {
                    if let Some(installed) = modules
                        .installed
                        .iter()
                        .find(|module| module.module_id.as_deref() == Some(id))
                    {
                        if usable(&installed) {
                            self.choice = Some(installed.file.clone());
                        }
                        self.awaiting = None;
                    }
                }
            }
        }
        self.modules = Some(modules);
    }

    fn online(&self, cx: &Context<Self>) -> bool {
        *self.daemon.read(cx).connection() == Connection::Online
    }

    /// What to call the module chosen.
    fn chosen_name(&self) -> Option<String> {
        let file = self.choice.as_deref()?;
        let name = self
            .modules
            .as_ref()
            .and_then(|modules| modules.installed.iter().find(|module| module.file == file))
            .map(|module| module.name.clone())
            .unwrap_or_else(|| vrft_protocol::module_name(file));
        Some(name)
    }

    /// Sends one request about modules, then shows what VRFT answers and
    /// runs `then` if it worked.
    fn request(
        &mut self,
        send: impl FnOnce(&DaemonClient) -> anyhow::Result<Modules> + Send + 'static,
        then: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.pending {
            return;
        }
        self.pending = true;
        self.message = None;
        cx.notify();
        let client = self.daemon.read(cx).client();
        self._request = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { send(&client) })
                .await;
            this.update(cx, |page, cx| {
                page.pending = false;
                match result {
                    Ok(modules) => {
                        page.apply_modules(modules);
                        then(page, cx);
                    }
                    Err(error) => {
                        page.awaiting = None;
                        page.message = Some(Notice::new(Tone::Problem, format!("{error:#}")));
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Installs registry module `id`, to choose it once it's ready.
    fn install(&mut self, id: String, cx: &mut Context<Self>) {
        self.awaiting = Some(id.clone());
        self.request(move |client| client.install_module(&id), |_, _| {}, cx);
    }

    fn choose(&mut self, file: String, cx: &mut Context<Self>) {
        self.choice = Some(file);
        self.message = None;
        cx.notify();
    }

    /// The primary button: on to the next step, switching VRFT to the
    /// module chosen on the way out of that step, or finishing.
    fn forward(&mut self, cx: &mut Context<Self>) {
        if self.step == self.done_step() {
            self.finish(cx);
        } else if self.step == MODULE {
            self.continue_from_modules(cx);
        } else {
            self.go_to(self.step + 1, cx);
        }
    }

    /// Leaves the module step: VRFT switches to the module chosen, if it
    /// isn't the one in use already.
    fn continue_from_modules(&mut self, cx: &mut Context<Self>) {
        let active = self
            .modules
            .as_ref()
            .map(|modules| modules.active.clone())
            .unwrap_or_default();
        match self.choice.clone().filter(|file| *file != active) {
            Some(file) => {
                self.request(
                    move |client| client.use_module(&file),
                    |page, cx| page.go_to(MODULE + 1, cx),
                    cx,
                );
            }
            None => self.go_to(MODULE + 1, cx),
        }
    }

    fn go_to(&mut self, step: usize, cx: &mut Context<Self>) {
        self.step = step.min(self.done_step());
        self.message = None;
        cx.notify();
    }

    /// Whether extension `id` runs: as VRFT says, or as `config.json` says
    /// while VRFT can't.
    fn extension_on(&self, id: &str, cx: &Context<Self>) -> bool {
        self.daemon
            .read(cx)
            .extension(id)
            .map(|extension| extension.enabled)
            .unwrap_or_else(|| self.extensions.read(cx).configured(id))
    }

    /// Whether offer `index`'s extension will run once setup finishes.
    fn will_be_on(&self, index: usize, cx: &Context<Self>) -> bool {
        self.answers[index].unwrap_or_else(|| self.extension_on(self.offers[index].id, cx))
    }

    /// Ends setup without answering anything more. The module, if one was
    /// chosen, stays chosen.
    fn skip(&mut self, cx: &mut Context<Self>) {
        self.answers = vec![None; self.offers.len()];
        self.finish(cx);
    }

    /// Turns the add-ons on or off as answered, notes that setup is done
    /// and opens the first add-on said yes to, or Home. Tracking restarts
    /// if an add-on it's running with changed.
    fn finish(&mut self, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        let online = self.online(cx);
        let changes: Vec<(&'static str, bool)> = self
            .offers
            .iter()
            .zip(&self.answers)
            .filter_map(|(offer, answer)| {
                let wanted = (*answer)?;
                (wanted != self.extension_on(offer.id, cx)).then_some((offer.id, wanted))
            })
            .collect();
        let next = self
            .offers
            .iter()
            .zip(&self.answers)
            .find(|(_, answer)| **answer == Some(true))
            .map_or(PageId::HOME, |(offer, _)| offer.offer.next_page);
        self.pending = true;
        self.message = None;
        cx.notify();
        let client = self.daemon.read(cx).client();
        let saving = changes.clone();
        self._request = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    for (id, enabled) in saving {
                        if online {
                            client.set_extension_enabled(id, enabled)?;
                        } else {
                            config_file::set_extension_enabled(id, enabled)?;
                        }
                    }
                    if online {
                        client.update_config(&ConfigPatch {
                            setup_done: Some(true),
                            ..ConfigPatch::default()
                        })?;
                    } else {
                        config_file::set_setup_done()?;
                    }
                    anyhow::Ok(())
                })
                .await;
            this.update(cx, |page, cx| {
                page.pending = false;
                match result {
                    Ok(()) => {
                        page.extensions.update(cx, |extensions, cx| {
                            for (id, enabled) in &changes {
                                extensions.set_configured(id, *enabled);
                            }
                            cx.notify();
                        });
                        if online && !changes.is_empty() {
                            page.launcher
                                .update(cx, |launcher, cx| launcher.restart(cx));
                        }
                        nav::open_page(next, cx);
                    }
                    Err(error) => {
                        page.message = Some(Notice::error(&t!("setup.save_failed"), &error));
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// The rail: VRFT's mark, the steps, and Skip setup at the foot.
    fn rail(&self, cx: &Context<Self>) -> impl IntoElement {
        let done_step = self.done_step();
        let mut steps: Vec<(SharedString, Option<SharedString>)> = vec![
            (t!("setup.rail_welcome").into(), None),
            (
                t!("setup.rail_module").into(),
                if self.step > MODULE || self.choice.is_some() {
                    Some(
                        self.chosen_name()
                            .map(SharedString::from)
                            .unwrap_or_else(|| t!("setup.none_chosen").into()),
                    )
                } else {
                    None
                },
            ),
        ];
        for (index, offer) in self.offers.iter().enumerate() {
            let answer = self.answers[index].map(|yes| {
                if yes {
                    t!("setup.rail_on").into()
                } else {
                    t!("setup.rail_off").into()
                }
            });
            steps.push((t!("setup.add_on", name = offer.name).into(), answer));
        }
        steps.push((t!("setup.rail_done").into(), None));
        let count = steps.len();
        let rows: Vec<AnyElement> = steps
            .into_iter()
            .enumerate()
            .map(|(index, (title, detail))| {
                let progress = if index < self.step {
                    Progress::Done
                } else if index == self.step {
                    Progress::Current
                } else {
                    Progress::Upcoming
                };
                let last = index + 1 == count;
                // Steps already done can be gone back to.
                let can_return = progress == Progress::Done && !self.pending;
                h_flex()
                    .id(("setup-rail-step", index))
                    .items_start()
                    .gap_3()
                    .px_2()
                    .rounded(px(8.))
                    .when(can_return, |row| {
                        row.cursor_pointer()
                            .hover(|style| style.bg(gpui_kit::rgb(0x17171a)))
                            .on_click(cx.listener(move |page, _, _, cx| page.go_to(index, cx)))
                    })
                    .child(
                        v_flex()
                            .items_center()
                            .pt(px(4.))
                            .child(step_ring(index + 1, progress))
                            .when(!last, |column| {
                                column.child(
                                    div().w(px(2.)).h(px(30.)).my(px(4.)).rounded_full().bg(
                                        if progress == Progress::Done {
                                            palette::text_3()
                                        } else {
                                            palette::line_soft()
                                        },
                                    ),
                                )
                            }),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .pt(px(7.))
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(13.5))
                                    .truncate()
                                    .when(progress == Progress::Current, |text| {
                                        text.font_semibold().text_color(palette::text())
                                    })
                                    .when(progress == Progress::Done, |text| {
                                        text.text_color(palette::text_2())
                                    })
                                    .when(progress == Progress::Upcoming, |text| {
                                        text.text_color(palette::text_3())
                                    })
                                    .child(title),
                            )
                            .children(detail.map(|detail| {
                                div()
                                    .text_xs()
                                    .text_color(palette::text_3())
                                    .truncate()
                                    .child(detail)
                            })),
                    )
                    .into_any_element()
            })
            .collect();
        let finishing = self.step == done_step;
        v_flex()
            .flex_none()
            .h_full()
            .w(px(RAIL_WIDTH))
            .bg(palette::rail())
            .border_r_1()
            .border_color(gpui_kit::rgb(0x1e1e22))
            .px(px(14.))
            .pt(px(10.))
            .pb(px(16.))
            .child(
                h_flex()
                    .id("setup-brand")
                    .h(px(36.))
                    .gap(px(10.))
                    .pl(px(8.))
                    .window_control_area(WindowControlArea::Drag)
                    .child(mark_tile(26.))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(14.5))
                            .font_semibold()
                            .child(crate::updates::app_name()),
                    ),
            )
            .child(
                div()
                    .mt(px(30.))
                    .mb_3()
                    .px_2()
                    .child(cap(t!("setup.rail_title"))),
            )
            .child(v_flex().children(rows))
            .child(div().flex_1())
            .when(!finishing, |rail| {
                rail.child(
                    div().px_1().child(
                        Button::new("setup-skip")
                            .ghost()
                            .small()
                            .label(t!("setup.skip"))
                            .tooltip(t!("setup.skip_tooltip"))
                            .disabled(self.pending)
                            .on_click(cx.listener(|page, _, _, cx| page.skip(cx))),
                    ),
                )
            })
    }

    /// The first step: what VRFaceTracking does, and what setup asks.
    fn welcome_step(&self, total: usize) -> AnyElement {
        let mut ahead: Vec<(IconName, SharedString, SharedString)> = vec![(
            IconName::Package,
            t!("setup.ahead_module").into(),
            t!("setup.ahead_module_detail").into(),
        )];
        for offer in &self.offers {
            ahead.push((
                offer.icon,
                t!("setup.add_on", name = offer.name).into(),
                t!("setup.ahead_offer_detail", name = offer.name).into(),
            ));
        }
        ahead.push((
            IconName::Check,
            t!("setup.ahead_done").into(),
            t!("setup.ahead_done_detail").into(),
        ));
        let rows = ahead
            .into_iter()
            .enumerate()
            .map(|(index, (icon, title, detail))| {
                h_flex()
                    .gap(px(14.))
                    .px_5()
                    .py(px(14.))
                    .when(index > 0, |row| {
                        row.border_t_1().border_color(palette::line_soft())
                    })
                    .child(icon_tile(icon, false, 34.).text_color(palette::text()))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.))
                            .child(div().text_sm().font_medium().child(title))
                            .child(div().text_xs().text_color(palette::text_3()).child(detail)),
                    )
            });
        v_flex()
            .gap_8()
            .child(v_flex().gap_5().child(mark_tile(64.)).child(step_header(
                1,
                total,
                t!("setup.welcome_title"),
                t!("setup.welcome_description"),
            )))
            .child(
                Section::new(t!("setup.ahead"))
                    .aside(t!("setup.ahead_aside"))
                    .child(card_column().children(rows)),
            )
            .into_any_element()
    }

    /// A module on this PC.
    fn installed_row(
        &self,
        index: usize,
        count: usize,
        module: &InstalledModule,
        modules: &Modules,
        cx: &Context<Self>,
    ) -> AnyElement {
        let blocked = unusable(module, modules);
        let chosen = self.choice.as_deref() == Some(module.file.as_str());
        let bundled = module.runtime != "dotnet" && !module.removable && module.module_id.is_none();
        let detail = h_flex()
            .gap_1()
            .min_w_0()
            .text_xs()
            .text_color(palette::text_3())
            .child(div().flex_none().child(runtime_label(&module.runtime)))
            .when_some(module.version.clone(), |line, version| {
                line.child("\u{b7}")
                    .child(mono(version).min_w_0().truncate())
            })
            .when(module.version.is_none() && bundled, |line| {
                line.child("\u{b7}").child(
                    div()
                        .min_w_0()
                        .truncate()
                        .child(t!("modules.comes_with_vrft")),
                )
            });
        let file = module.file.clone();
        let can_choose = blocked.is_none() && !self.pending;
        choice_row(("setup-installed", index), index, count, chosen)
            .when(can_choose, |row| {
                row.cursor_pointer()
                    .hover(|style| style.bg(palette::inset()))
                    .on_click(cx.listener(move |page, _, _, cx| page.choose(file.clone(), cx)))
            })
            .child(module_tile(chosen, TILE))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_sm()
                            .font_medium()
                            .truncate()
                            .child(module.name.clone()),
                    )
                    .child(detail)
                    .children(blocked.map(|reason| {
                        div()
                            .text_xs()
                            .text_color(palette::signal_text())
                            .child(reason)
                    })),
            )
            .child(radio(chosen))
            .into_any_element()
    }

    /// A module in the registry: install it, or choose it once installed.
    fn registry_row(
        &self,
        index: usize,
        first: bool,
        entry: &RegistryModule,
        modules: &Modules,
        cx: &Context<Self>,
    ) -> AnyElement {
        let installed = modules
            .installed
            .iter()
            .find(|module| module.module_id.as_deref() == Some(&entry.module_id));
        let operation = modules
            .operations
            .iter()
            .find(|operation| operation.module_id == entry.module_id);
        let running = operation.is_some_and(|operation| operation.state.running());
        let chosen = installed.is_some_and(|module| self.choice.as_deref() == Some(&module.file));
        let selectable = installed
            .filter(|module| unusable(module, modules).is_none() && !running && !self.pending)
            .map(|module| module.file.clone());
        let id = entry.module_id.clone();
        let action = match (operation.filter(|_| running), installed) {
            (Some(operation), _) => progress(operation).into_any_element(),
            (None, Some(_)) => radio(chosen).into_any_element(),
            (None, None) => Button::new(("setup-install", index))
                .small()
                .h_7()
                .px_2p5()
                .icon(IconName::Download)
                .label(t!("modules.install"))
                .loading(self.pending && self.awaiting.as_deref() == Some(id.as_str()))
                .disabled(self.pending || self.awaiting.is_some())
                .on_click(cx.listener(move |page, _, _, cx| page.install(id.clone(), cx)))
                .into_any_element(),
        };
        let failed = operation
            .filter(|operation| operation.state == OperationState::Failed)
            .map(|operation| {
                Notice::new(
                    Tone::Problem,
                    t!("modules.install_failed", reason = operation.message),
                )
            });
        h_flex()
            .id(("setup-registry-row", index))
            .items_start()
            .gap(px(14.))
            .px_4()
            .py(px(12.))
            .when(!first, |row| {
                row.border_t_1().border_color(palette::line_soft())
            })
            .when(chosen, |row| row.bg(palette::inset()))
            .when_some(selectable, |row, file| {
                row.cursor_pointer()
                    .hover(|style| style.bg(palette::inset()))
                    .on_click(cx.listener(move |page, _, _, cx| page.choose(file.clone(), cx)))
            })
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.))
                    .child(
                        div()
                            .text_size(px(13.5))
                            .font_medium()
                            .truncate()
                            .child(entry.module_name.clone()),
                    )
                    .when(!entry.module_description.is_empty(), |text| {
                        text.child(
                            div()
                                .text_xs()
                                .text_color(palette::text_3())
                                .child(entry.module_description.clone()),
                        )
                    })
                    .child(
                        h_flex()
                            .gap_1()
                            .min_w_0()
                            .text_size(px(11.))
                            .text_color(palette::text_4())
                            .child(mono(entry.version.clone()).flex_none())
                            .when(!entry.author_name.is_empty(), |line| {
                                line.child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .child(t!("modules.by_author", author = entry.author_name)),
                                )
                            }),
                    )
                    .children(failed.map(|notice| div().pt(px(3.)).child(notice))),
            )
            .child(action)
            .into_any_element()
    }

    /// The registry, to search and install from.
    fn registry_card(&self, modules: &Modules, cx: &Context<Self>) -> impl IntoElement {
        let query = self.search.read(cx).value().to_string();
        let registry = &modules.registry;
        let rows: Vec<AnyElement> = registry
            .modules
            .iter()
            .enumerate()
            .filter(|(_, entry)| matches(entry, &query))
            .enumerate()
            .map(|(shown, (index, entry))| self.registry_row(index, shown == 0, entry, modules, cx))
            .collect();
        card(cx)
            .flex()
            .flex_col()
            .min_w_0()
            .child(
                v_flex()
                    .gap_3()
                    .p_4()
                    .child(
                        Input::new(&self.search)
                            .prefix(
                                div()
                                    .text_color(palette::text_3())
                                    .child(Icon::new(IconName::Search).size(px(15.))),
                            )
                            .bg(palette::sunken()),
                    )
                    .when_some(registry.error.clone(), |block, error| {
                        block.child(Notice::new(
                            Tone::Problem,
                            t!("modules.registry_failed", error = error),
                        ))
                    })
                    .when(registry.loading && registry.modules.is_empty(), |block| {
                        block.child(hint(t!("modules.fetching_registry"), cx))
                    })
                    .when(!registry.modules.is_empty() && rows.is_empty(), |block| {
                        block.child(hint(t!("modules.no_matches"), cx))
                    }),
            )
            .when(!rows.is_empty(), |card| {
                card.child(
                    v_flex()
                        .id("setup-registry")
                        .max_h(px(REGISTRY_HEIGHT))
                        .overflow_y_scroll()
                        .border_t_1()
                        .border_color(palette::line_soft())
                        .children(rows),
                )
            })
    }

    /// The module step: the modules on this PC, and the registry's.
    fn module_step(&self, total: usize, cx: &Context<Self>) -> AnyElement {
        let header = step_header(
            MODULE + 1,
            total,
            t!("setup.module_title"),
            t!("setup.module_description"),
        );
        let body = if !self.online(cx) {
            v_flex()
                .gap_3()
                .items_start()
                .child(hint(t!("setup.start_to_choose"), cx))
                .child(StartVrft::new(self.launcher.clone()))
                .into_any_element()
        } else if let Some(modules) = self.modules.as_ref() {
            let shown: Vec<&InstalledModule> = modules
                .installed
                .iter()
                .filter(|module| offered(module))
                .collect();
            let count = shown.len();
            let installed: Vec<AnyElement> = shown
                .into_iter()
                .enumerate()
                .map(|(index, module)| self.installed_row(index, count, module, modules, cx))
                .collect();
            v_flex()
                .gap_6()
                .child(
                    Section::new(t!("setup.on_this_pc"))
                        .aside(t!("setup.on_this_pc_aside"))
                        .when(installed.is_empty(), |section| {
                            section.child(hint(t!("setup.none_on_this_pc"), cx))
                        })
                        .when(!installed.is_empty(), |section| {
                            section.child(card_column().children(installed))
                        }),
                )
                .child(
                    Section::new(t!("setup.registry"))
                        .aside(t!("setup.registry_aside"))
                        .child(self.registry_card(modules, cx)),
                )
                .into_any_element()
        } else {
            hint(t!("modules.reading"), cx).into_any_element()
        };
        v_flex()
            .gap_8()
            .child(header)
            .child(body)
            .into_any_element()
    }

    /// A large tile answering an add-on's question yes or no.
    fn answer_tile(
        &self,
        offer: usize,
        yes: bool,
        icon: IconName,
        title: SharedString,
        detail: SharedString,
        cx: &Context<Self>,
    ) -> AnyElement {
        let chosen = self.answers[offer] == Some(yes);
        v_flex()
            .id(("setup-answer", usize::from(yes)))
            .flex_1()
            .min_w_0()
            .gap_3()
            .p_5()
            .rounded(px(14.))
            .border_1()
            .border_color(if chosen {
                palette::text()
            } else {
                palette::line()
            })
            .bg(if chosen {
                palette::inset()
            } else {
                palette::surface()
            })
            .when(!self.pending, |tile| {
                tile.cursor_pointer()
                    .hover(|style| style.border_color(palette::line_focus()))
                    .on_click(cx.listener(move |page, _, _, cx| {
                        page.answers[offer] = Some(yes);
                        cx.notify();
                    }))
            })
            .child(
                h_flex()
                    .justify_between()
                    .child(icon_tile(icon, chosen, 36.))
                    .child(radio(chosen)),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().font_semibold().child(title))
                    .child(div().text_xs().text_color(palette::text_3()).child(detail)),
            )
            .into_any_element()
    }

    /// A step asking whether to turn an add-on on: what it adds, then yes
    /// or no.
    fn offer_step(&self, index: usize, total: usize, cx: &Context<Self>) -> AnyElement {
        let offer = &self.offers[index];
        let features = offer.offer.features.iter().map(|feature| {
            h_flex()
                .items_start()
                .gap_2p5()
                .text_size(px(13.))
                .text_color(palette::text_2())
                .child(
                    div()
                        .pt(px(2.))
                        .text_color(palette::good())
                        .child(Icon::new(IconName::Check).size(px(14.))),
                )
                .child(div().flex_1().min_w_0().child(feature.clone()))
        });
        let what = card(cx).p_5().child(
            v_flex()
                .gap_3()
                .child(
                    h_flex()
                        .gap_3()
                        .child(icon_tile(offer.icon, false, 36.).text_color(palette::text()))
                        .child(
                            v_flex()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .text_sm()
                                        .font_semibold()
                                        .child(t!("setup.add_on", name = offer.name)),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(palette::text_3())
                                        .child(t!("setup.works_alongside")),
                                ),
                        ),
                )
                .child(v_flex().gap_2().pt_1().children(features))
                .children(offer.offer.requirements.clone().map(|requirements| {
                    h_flex()
                        .items_start()
                        .gap_2p5()
                        .mt_1()
                        .pt_3()
                        .border_t_1()
                        .border_color(palette::line_soft())
                        .text_xs()
                        .text_color(palette::text_3())
                        .child(
                            div()
                                .pt(px(1.))
                                .child(Icon::new(IconName::Info).size(px(14.))),
                        )
                        .child(div().flex_1().min_w_0().child(requirements))
                })),
        );
        v_flex()
            .gap_8()
            .child(step_header(
                MODULE + 2 + index,
                total,
                offer.offer.question.clone(),
                t!("setup.offer_description", name = offer.name),
            ))
            .child(what)
            .child(
                h_flex()
                    .gap_3()
                    .items_stretch()
                    .child(self.answer_tile(
                        index,
                        true,
                        offer.icon,
                        t!("setup.yes", name = offer.name).into(),
                        t!("setup.yes_detail").into(),
                        cx,
                    ))
                    .child(self.answer_tile(
                        index,
                        false,
                        IconName::CircleX,
                        t!("setup.no", name = offer.name).into(),
                        t!("setup.no_detail").into(),
                        cx,
                    )),
            )
            .into_any_element()
    }

    /// The last step: how VRFT is set up, and what comes next.
    fn done_step_view(&self, total: usize, cx: &Context<Self>) -> AnyElement {
        let mut rows = vec![summary_row(
            0,
            t!("setup.summary_module").into(),
            self.chosen_name()
                .map(SharedString::from)
                .unwrap_or_else(|| t!("setup.summary_no_module").into()),
            self.choice.is_some(),
        )];
        for (index, offer) in self.offers.iter().enumerate() {
            let on = self.will_be_on(index, cx);
            rows.push(summary_row(
                index + 1,
                t!("setup.add_on", name = offer.name).into(),
                if on {
                    t!("setup.summary_on").into()
                } else {
                    t!("setup.summary_off").into()
                },
                on,
            ));
        }
        let next = self
            .offers
            .iter()
            .zip(&self.answers)
            .find(|(_, answer)| **answer == Some(true))
            .map(|(offer, _)| {
                t!(
                    "setup.next_offer",
                    name = offer.name,
                    page = offer.offer.next_page.label()
                )
            })
            .unwrap_or_else(|| {
                if self.choice.is_some() {
                    t!("setup.next_tracking")
                } else {
                    t!("setup.next_no_module")
                }
            });
        v_flex()
            .gap_8()
            .child(
                v_flex()
                    .gap_5()
                    .child(hero_badge(
                        IconName::Check,
                        palette::good(),
                        palette::good_bg(),
                        palette::good_line(),
                    ))
                    .child(step_header(
                        total,
                        total,
                        t!("setup.done_title"),
                        t!("setup.done_description"),
                    )),
            )
            .child(card_column().children(rows))
            .child(
                h_flex()
                    .items_start()
                    .gap_3()
                    .px_5()
                    .py_4()
                    .rounded(px(14.))
                    .bg(palette::inset())
                    .text_size(px(13.))
                    .text_color(palette::text_2())
                    .child(
                        div()
                            .pt(px(1.))
                            .text_color(palette::text())
                            .child(Icon::new(IconName::ArrowRight).size(px(15.))),
                    )
                    .child(div().flex_1().min_w_0().child(next)),
            )
            .into_any_element()
    }

    /// Back, and the step's primary button, pinned under the step.
    fn footer(&self, cx: &Context<Self>) -> impl IntoElement {
        let done_step = self.done_step();
        let (label, ready) = if self.step == WELCOME {
            (t!("setup.get_started"), true)
        } else if self.step == MODULE {
            match self.choice {
                None => (
                    t!("setup.continue_without_module"),
                    self.online(cx) && self.awaiting.is_none(),
                ),
                Some(_) => (t!("setup.continue"), self.awaiting.is_none()),
            }
        } else if self.step == done_step {
            (t!("setup.finish"), true)
        } else {
            let answered = self
                .offer_index(self.step)
                .is_some_and(|index| self.answers[index].is_some());
            (t!("setup.continue"), answered)
        };
        let forward = Button::new("setup-next")
            .primary()
            .label(label)
            .loading(self.pending && self.awaiting.is_none())
            .disabled(self.pending || !ready)
            .on_click(cx.listener(|page, _, _, cx| page.forward(cx)));
        h_flex()
            .gap_2()
            .items_center()
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_3())
                    .when(self.step == WELCOME, |text| {
                        text.child(t!("setup.takes_a_minute"))
                    }),
            )
            .child(div().flex_1())
            .when(self.step > WELCOME, |row| {
                row.child(
                    Button::new("setup-back")
                        .ghost()
                        .icon(IconName::ArrowLeft)
                        .label(t!("setup.back"))
                        .disabled(self.pending)
                        .on_click(cx.listener(|page, _, _, cx| page.go_to(page.step - 1, cx))),
                )
            })
            .child(forward)
    }
}

/// A card holding rows that fill it.
fn card_column() -> Div {
    div()
        .flex()
        .flex_col()
        .rounded(px(14.))
        .border_1()
        .border_color(palette::line())
        .bg(palette::surface())
}

/// Lines a step's content, or its footer, up in one centred column.
fn column() -> Div {
    div()
        .w_full()
        .max_w(px(CONTENT_WIDTH))
        .mx_auto()
        .px(px(PAGE_PADDING))
}

impl Render for SetupPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let total = self.done_step() + 1;
        let body = if self.step == WELCOME {
            self.welcome_step(total)
        } else if self.step == MODULE {
            self.module_step(total, cx)
        } else if let Some(index) = self.offer_index(self.step) {
            self.offer_step(index, total, cx)
        } else {
            self.done_step_view(total, cx)
        };
        h_flex()
            .size_full()
            .items_stretch()
            .child(self.rail(cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(WindowStrip)
                    .child(
                        // Each step starts at its top.
                        div()
                            .id(("setup-step", self.step))
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(column().pt(px(18.)).pb(px(32.)).child(
                                v_flex().gap_6().children(self.message.clone()).child(body),
                            )),
                    )
                    .child(
                        div()
                            .flex_none()
                            .py_4()
                            .border_t_1()
                            .border_color(palette::line_soft())
                            .child(column().child(self.footer(cx))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_while_fetching_or_installing() {
        assert!(!busy(None));
        let mut modules = Modules::default();
        assert!(!busy(Some(&modules)));
        modules.operations.push(ModuleOperation {
            state: OperationState::Downloading,
            ..ModuleOperation::default()
        });
        assert!(busy(Some(&modules)));
    }

    #[test]
    fn setup_leaves_out_the_test_logger() {
        let module = |file: &str| InstalledModule {
            file: file.into(),
            ..InstalledModule::default()
        };
        assert!(offered(&module("vd_module.dll")));
        assert!(!offered(&module("test_logger.dll")));
    }
}
