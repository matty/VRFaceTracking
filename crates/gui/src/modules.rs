//! Modules: the tracking modules in VRFT's plugins folder, and the VRCFT
//! module registry to install more from. VRFT does the downloading and
//! installing in the background; this page starts it and follows along.
//! Using a module, native or VRCFT (.NET), switches VRFT to it in place,
//! without restarting.
use crate::extension_switches::ExtensionSwitches;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex, Disableable as _, Icon, Sizable as _, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, AnyElement, AppContext as _, Context, Div, Entity, Hsla, InteractiveElement as _,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement as _, Styled,
    Subscription, Task, Window,
};
use rust_i18n::t;
use std::sync::Arc;
use std::time::Duration;
use vrft_gui_core::client::{
    DaemonClient, InstalledModule, ModuleOperation, ModuleStatus, Modules, OperationState,
    RegistryModule, RunMode,
};
use vrft_gui_core::launcher::{Launcher, StartVrft};
use vrft_gui_core::live::DaemonState;
use vrft_gui_core::palette::{self, MONO_FONT};
use vrft_gui_core::summary::{module_failure, module_state, Connection, Tone};
use vrft_gui_core::widgets::{
    cap, card, hint, mono, Meter, Notice, PageHeader, Section, StatusPill,
};

/// How often the page reads `/modules` while it shows.
const POLL: Duration = Duration::from_secs(2);
/// How often while something is downloading or installing.
const BUSY_POLL: Duration = Duration::from_millis(300);

/// Narrowest the content gets with Get modules beside the installed ones;
/// below it they stack.
const TWO_COLUMNS_MIN: f32 = 860.;
/// Get modules' column, beside the installed ones.
const REGISTRY_WIDTH: f32 = 400.;
/// An installed module's icon tile, and the gap after it. What shows under a
/// row lines up past them, with the row's text.
const TILE: f32 = 36.;
const ROW_GAP: f32 = 14.;
/// A card's corner inside its 1px edge, for a row that fills it.
const INNER_RADIUS: f32 = 13.;

/// A request this page is waiting on, so its button shows it.
#[derive(Clone, PartialEq)]
enum Pending {
    Refresh,
    Install(String),
    Remove(String),
    Use(String),
}

pub struct ModulesPage {
    daemon: Entity<DaemonState>,
    launcher: Entity<Launcher>,
    /// The add-ons' switches.
    switches: Entity<ExtensionSwitches>,
    modules: Option<Modules>,
    search: Entity<InputState>,
    /// Why the last request failed, or what it did.
    message: Option<Notice>,
    pending: Option<Pending>,
    /// Remove was pressed once for this module; pressing again removes it.
    confirm_remove: Option<String>,
    watching: bool,
    _poll: Task<()>,
    _request: Option<Task<()>>,
    _subscriptions: [Subscription; 3],
}

/// Whether `modules` has something running that the page should follow
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

/// Whether `entry` matches the search text.
fn matches(entry: &RegistryModule, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    query.is_empty()
        || [
            &entry.module_name,
            &entry.author_name,
            &entry.module_description,
        ]
        .iter()
        .any(|text| text.to_lowercase().contains(&query))
}

/// "VRCFT module" or "Native module".
fn runtime_label(runtime: &str) -> SharedString {
    if runtime == "dotnet" {
        t!("modules.vrcft_module").into()
    } else {
        t!("modules.native_module").into()
    }
}

/// Why `module` can't be used, when it can't.
fn unusable(module: &InstalledModule, modules: &Modules) -> Option<SharedString> {
    (module.runtime == "dotnet" && !modules.dotnet_host)
        .then(|| t!("modules.needs_dotnet_host").into())
}

/// A module's square icon tile: white for the one in use, grey otherwise.
/// Settings shows the same tile for the module it names.
pub(crate) fn module_tile(in_use: bool, size: f32) -> Div {
    icon_tile(IconName::Package, in_use, size)
}

/// A square tile behind an icon, as at the start of a module's or an
/// add-on's row: white with a black icon when `lit`, grey otherwise.
pub(crate) fn icon_tile(icon: IconName, lit: bool, size: f32) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(size))
        .rounded(px((size / 4.).round()))
        .bg(if lit {
            palette::text()
        } else {
            Hsla::from(gpui_kit::rgb(0x1f1f23))
        })
        .text_color(if lit {
            palette::page()
        } else {
            palette::text_2()
        })
        .child(Icon::new(icon).size(px((size * 0.47).round())))
}

/// A small button in a row, 28px high.
fn row_button(button: Button) -> Button {
    button.small().h_7().px_2p5()
}

/// A 28px square button with only an icon, named by its tooltip.
fn icon_button(
    id: (&'static str, usize),
    icon: IconName,
    tooltip: impl Into<SharedString>,
) -> Button {
    let tooltip = tooltip.into();
    Button::new(id)
        .ghost()
        .small()
        .size_7()
        .icon(icon)
        .tooltip(tooltip.clone())
        .accessibility_label(tooltip)
}

/// What shows under a row, lined up with its text rather than its icon.
fn under_row(children: Vec<AnyElement>) -> Option<Div> {
    (!children.is_empty()).then(|| v_flex().gap_2().pl(px(TILE + ROW_GAP)).children(children))
}

/// A notice with the button that deals with it beside it.
fn notice_with(notice: Notice, button: Button) -> AnyElement {
    h_flex()
        .gap_2()
        .items_start()
        .child(div().flex_1().min_w_0().child(notice))
        .child(button)
        .into_any_element()
}

impl ModulesPage {
    pub fn new(
        daemon: Entity<DaemonState>,
        launcher: Entity<Launcher>,
        switches: Entity<ExtensionSwitches>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("modules.search_placeholder")));
        let client = daemon.read(cx).client();
        let poll = cx.spawn_in(window, async move |this, cx| loop {
            let Ok((read, fast)) = this.update(cx, |page, cx| {
                let online = *page.daemon.read(cx).connection() == Connection::Online;
                (page.watching && online, busy(page.modules.as_ref()))
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
                        Ok(modules) => page.modules = Some(modules),
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
        Self {
            daemon,
            launcher,
            switches,
            modules: None,
            search,
            message: None,
            pending: None,
            confirm_remove: None,
            watching: false,
            _poll: poll,
            _request: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn set_watching(&mut self, watching: bool, cx: &mut Context<Self>) {
        self.watching = watching;
        cx.notify();
    }

    /// Sends one request, then shows what VRFT answers. `done` is said when
    /// it worked.
    fn request(
        &mut self,
        pending: Pending,
        send: impl FnOnce(&DaemonClient) -> anyhow::Result<Modules> + Send + 'static,
        done: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.pending.is_some() {
            return;
        }
        self.pending = Some(pending);
        self.message = None;
        self.confirm_remove = None;
        cx.notify();
        let client: Arc<DaemonClient> = self.daemon.read(cx).client();
        self._request = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { send(&client) })
                .await;
            this.update(cx, |page, cx| {
                page.pending = None;
                match result {
                    Ok(modules) => {
                        page.modules = Some(modules);
                        page.message = done.map(|text| Notice::new(Tone::Good, text));
                    }
                    Err(error) => {
                        page.message = Some(Notice::new(Tone::Problem, format!("{error:#}")))
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.request(
            Pending::Refresh,
            |client| client.refresh_registry(),
            None,
            cx,
        );
    }

    fn install(&mut self, id: String, cx: &mut Context<Self>) {
        let request = id.clone();
        self.request(
            Pending::Install(id),
            move |client| client.install_module(&request),
            None,
            cx,
        );
    }

    fn remove(&mut self, id: String, name: String, cx: &mut Context<Self>) {
        let request = id.clone();
        self.request(
            Pending::Remove(id),
            move |client| client.uninstall_module(&request),
            Some(t!("modules.removed", name = name).into_owned()),
            cx,
        );
    }

    /// Makes `file` the tracking module. VRFT loads it in place of the one
    /// running, and `/status` shows how that goes.
    fn use_module(&mut self, file: String, name: String, cx: &mut Context<Self>) {
        let extensions_only = self.daemon_mode(cx) == Some(RunMode::ExtensionsOnly);
        let done = if extensions_only {
            t!("modules.chosen_extensions_only", name = name).into_owned()
        } else {
            t!("modules.switching_to", name = name).into_owned()
        };
        let request = file.clone();
        self.request(
            Pending::Use(file),
            move |client| client.use_module(&request),
            Some(done),
            cx,
        );
    }

    fn daemon_mode(&self, cx: &Context<Self>) -> Option<RunMode> {
        Some(self.daemon.read(cx).status()?.daemon.as_ref()?.mode)
    }

    /// What VRFT says about the module it's running or loading.
    fn module_status(&self, cx: &Context<Self>) -> Option<ModuleStatus> {
        self.daemon
            .read(cx)
            .status()?
            .daemon
            .as_ref()?
            .module
            .clone()
    }

    /// The Use button for the module with key `file`.
    fn use_button(
        &self,
        id: (&'static str, usize),
        file: String,
        name: String,
        blocked: Option<SharedString>,
        disabled: bool,
        cx: &Context<Self>,
    ) -> Button {
        let is_blocked = blocked.is_some();
        row_button(Button::new(id))
            .label(t!("modules.use"))
            .tooltip(blocked.unwrap_or_else(|| t!("modules.use_tooltip").into()))
            .loading(self.pending == Some(Pending::Use(file.clone())))
            .disabled(disabled || is_blocked)
            .on_click(
                cx.listener(move |page, _, _, cx| page.use_module(file.clone(), name.clone(), cx)),
            )
    }

    /// "Update to 1.4.0", installing the registry's version of `module_id`.
    fn update_button(
        &self,
        id: (&'static str, usize),
        module_id: String,
        version: String,
        disabled: bool,
        running: bool,
        cx: &Context<Self>,
    ) -> Button {
        row_button(Button::new(id))
            .ghost()
            .icon(IconName::Download)
            .label(t!("modules.update_to_version", version = version))
            .loading(running || self.pending == Some(Pending::Install(module_id.clone())))
            .disabled(disabled || running)
            .on_click(cx.listener(move |page, _, _, cx| page.install(module_id.clone(), cx)))
    }

    fn operation(&self, id: &str) -> Option<&ModuleOperation> {
        self.modules
            .as_ref()?
            .operations
            .iter()
            .find(|operation| operation.module_id == id)
    }

    /// An install's progress bar or outcome, under its row.
    fn operation_line(&self, operation: &ModuleOperation) -> AnyElement {
        match operation.state {
            OperationState::Downloading | OperationState::Installing => {
                let label = match (operation.state, operation.fraction) {
                    (OperationState::Downloading, Some(fraction)) => {
                        t!(
                            "modules.downloading_percent",
                            percent = format!("{:.0}", fraction * 100.)
                        )
                    }
                    (OperationState::Downloading, None) => t!("modules.downloading"),
                    _ => t!("modules.installing"),
                };
                h_flex()
                    .gap_2p5()
                    .pt(px(2.))
                    .child(div().flex_1().min_w_0().child(Meter::new(
                        operation.fraction.unwrap_or(0.),
                        palette::text(),
                    )))
                    .child(
                        div()
                            .flex_none()
                            .font_family(MONO_FONT)
                            .text_size(px(11.))
                            .text_color(palette::text_2())
                            .child(label),
                    )
                    .into_any_element()
            }
            OperationState::Done => {
                Notice::new(Tone::Good, operation.message.clone()).into_any_element()
            }
            OperationState::Failed => Notice::new(
                Tone::Problem,
                t!("modules.install_failed", reason = operation.message),
            )
            .into_any_element(),
        }
    }

    /// One installed module: its tile, name and kind, what can be done with
    /// it, and under it anything that needs saying.
    fn installed_row(
        &self,
        index: usize,
        count: usize,
        module: &InstalledModule,
        modules: &Modules,
        cx: &Context<Self>,
    ) -> AnyElement {
        let active = module.file == modules.active;
        let status = self.module_status(cx).filter(|_| active);
        let busy = self.pending.is_some() || self.launcher.read(cx).is_busy();
        let id = module.module_id.clone();
        let running = id
            .as_deref()
            .and_then(|id| self.operation(id))
            .filter(|operation| operation.state.running())
            .is_some();
        let confirming = id.is_some() && self.confirm_remove == id;

        // "VRCFT module · 1.3.2", or "Native module · comes with VRFT" for
        // one that isn't from the registry.
        let bundled = module.runtime != "dotnet" && !module.removable && id.is_none();
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

        let actions = h_flex()
            .gap_1()
            .flex_none()
            .items_center()
            .when_some(
                module.update.clone().zip(id.clone()),
                |row, (version, id)| {
                    row.child(self.update_button(
                        ("update-module", index),
                        id,
                        version,
                        busy,
                        running,
                        cx,
                    ))
                },
            )
            .child(if active {
                let (tone, label) = module_state(status.as_ref());
                StatusPill::new(tone, label).into_any_element()
            } else {
                self.use_button(
                    ("use-module", index),
                    module.file.clone(),
                    module.name.clone(),
                    unusable(module, modules),
                    busy || running,
                    cx,
                )
                .into_any_element()
            })
            .when(module.removable && !active, |row| {
                let id = id.clone().unwrap_or_default();
                let name = module.name.clone();
                row.child(if confirming {
                    row_button(Button::new(("confirm-remove-module", index)))
                        .danger()
                        .outline()
                        .label(t!("modules.remove"))
                        .loading(self.pending == Some(Pending::Remove(id.clone())))
                        .disabled(busy)
                        .on_click(cx.listener(move |page, _, _, cx| {
                            page.remove(id.clone(), name.clone(), cx)
                        }))
                } else {
                    icon_button(
                        ("remove-module", index),
                        IconName::Trash,
                        t!("modules.remove"),
                    )
                    .disabled(busy || running)
                    .on_click(cx.listener(move |page, _, _, cx| {
                        page.confirm_remove = Some(id.clone());
                        cx.notify();
                    }))
                })
            })
            .when_some(module.page_url.clone(), |row, url| {
                row.child(
                    icon_button(
                        ("module-page", index),
                        IconName::ExternalLink,
                        t!("modules.open_page"),
                    )
                    .on_click(move |_, _, cx| cx.open_url(&url)),
                )
            });

        let mut under = Vec::new();
        if module.pending_restart {
            let launcher = self.launcher.clone();
            under.push(notice_with(
                Notice::new(Tone::Waiting, t!("modules.update_downloaded")),
                row_button(Button::new(("restart-for-module", index)))
                    .icon(IconName::RotateCcw)
                    .label(t!("modules.restart_vrft"))
                    .disabled(busy)
                    .on_click(move |_, _, cx| {
                        launcher.update(cx, |launcher, cx| launcher.restart(cx))
                    }),
            ));
        }
        if let Some(error) = module_failure(status.as_ref()) {
            let file = module.file.clone();
            let name = module.name.clone();
            under.push(notice_with(
                Notice::new(Tone::Problem, error),
                row_button(Button::new(("retry-module", index)))
                    .icon(IconName::RotateCcw)
                    .label(t!("modules.try_again"))
                    .loading(self.pending == Some(Pending::Use(file.clone())))
                    .disabled(busy)
                    .on_click(cx.listener(move |page, _, _, cx| {
                        page.use_module(file.clone(), name.clone(), cx)
                    })),
            ));
        }
        if confirming {
            under.push(hint(t!("modules.remove_confirm_hint"), cx).into_any_element());
        }
        under.extend(
            id.as_deref()
                .and_then(|id| self.operation(id))
                .filter(|operation| operation.state != OperationState::Done)
                .map(|operation| self.operation_line(operation)),
        );

        v_flex()
            .gap_3()
            .px_4()
            .py(px(ROW_GAP))
            .when(index > 0, |row| {
                row.border_t_1().border_color(palette::line_soft())
            })
            .when(active, |row| {
                row.bg(palette::inset())
                    .when(index == 0, |row| row.rounded_t(px(INNER_RADIUS)))
                    .when(index + 1 == count, |row| row.rounded_b(px(INNER_RADIUS)))
            })
            .child(
                h_flex()
                    .gap(px(ROW_GAP))
                    .child(module_tile(active, TILE))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_sm()
                                    .when(active, |name| name.font_semibold())
                                    .when(!active, |name| name.font_medium())
                                    .truncate()
                                    .child(module.name.clone()),
                            )
                            .child(detail),
                    )
                    .child(actions),
            )
            .children(under_row(under))
            .into_any_element()
    }

    /// One module in the registry: what it is, and installing or updating
    /// it. The rest of what the registry says shows on hover.
    fn registry_row(
        &self,
        index: usize,
        first: bool,
        entry: &RegistryModule,
        installed: Option<&InstalledModule>,
        modules: &Modules,
        cx: &Context<Self>,
    ) -> AnyElement {
        let busy = self.pending.is_some();
        let operation = self.operation(&entry.module_id);
        let running = operation.is_some_and(|operation| operation.state.running());
        let id = entry.module_id.clone();
        let action = match installed {
            // Its progress shows under it instead.
            _ if running => None,
            Some(installed) if installed.update.is_none() => {
                let label = if installed.file == modules.active {
                    t!("modules.in_use")
                } else {
                    t!("modules.installed")
                };
                Some(
                    h_flex()
                        .h_7()
                        .gap_1p5()
                        .text_xs()
                        .text_color(palette::text_2())
                        .child(Icon::new(IconName::Check).size(px(13.)))
                        .child(label)
                        .into_any_element(),
                )
            }
            Some(_) => Some(
                self.update_button(
                    ("update-registry-module", index),
                    id,
                    entry.version.clone(),
                    busy,
                    running,
                    cx,
                )
                .into_any_element(),
            ),
            None => Some(
                row_button(Button::new(("install-module", index)))
                    .label(t!("modules.install"))
                    .loading(self.pending == Some(Pending::Install(id.clone())))
                    .disabled(busy)
                    .on_click(cx.listener(move |page, _, _, cx| page.install(id.clone(), cx)))
                    .into_any_element(),
            ),
        };

        // What the registry says beyond the one line, for the tooltip.
        let mut about: Vec<SharedString> = Vec::new();
        if !entry.module_description.is_empty() {
            about.push(entry.module_description.clone().into());
        }
        if !entry.usage_instructions.is_empty() {
            about.push(entry.usage_instructions.clone().into());
        }
        let mut stats = vec![t!("modules.downloads", count = entry.downloads).into_owned()];
        if entry.ratings > 0 {
            stats.push(format!("\u{2605} {:.1}", entry.rating));
        }
        about.push(stats.join(" \u{b7} ").into());

        let page_url = Some(entry.module_page_url.clone()).filter(|url| !url.is_empty());
        h_flex()
            .items_start()
            .gap(px(ROW_GAP))
            .px_4()
            .py(px(ROW_GAP))
            .when(!first, |row| {
                row.border_t_1().border_color(palette::line_soft())
            })
            .child(
                v_flex()
                    .id(("registry-about", index))
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.))
                    .tooltip(move |window, cx| {
                        let about = about.clone();
                        Tooltip::element(move |_, _| {
                            v_flex()
                                .max_w(px(320.))
                                .gap_1p5()
                                .py_1()
                                .children(about.iter().map(|text| div().child(text.clone())))
                        })
                        .build(window, cx)
                    })
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
                                .truncate()
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
                    .children(
                        operation.map(|operation| {
                            div().pt(px(3.)).child(self.operation_line(operation))
                        }),
                    ),
            )
            .child(
                h_flex()
                    .gap_1()
                    .flex_none()
                    .items_center()
                    .children(action)
                    .when_some(page_url, |row, url| {
                        row.child(
                            icon_button(
                                ("registry-page", index),
                                IconName::ExternalLink,
                                t!("modules.open_page"),
                            )
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                        )
                    }),
            )
            .into_any_element()
    }

    /// "Installed" and its count, then the modules in one card.
    fn installed_section(&self, modules: &Modules, cx: &Context<Self>) -> impl IntoElement {
        let count = modules.installed.len();
        let rows = modules
            .installed
            .iter()
            .enumerate()
            .map(|(index, module)| self.installed_row(index, count, module, modules, cx))
            .collect::<Vec<_>>();
        let missing_host = !modules.dotnet_host
            && modules
                .installed
                .iter()
                .any(|module| module.runtime == "dotnet");
        v_flex()
            .gap_3()
            .child(
                h_flex().gap_2().child(cap(t!("modules.installed"))).child(
                    div()
                        .font_family(MONO_FONT)
                        .text_size(px(11.))
                        .text_color(palette::text_4())
                        .child(count.to_string()),
                ),
            )
            .when(missing_host, |section| {
                section.child(Notice::new(
                    Tone::Problem,
                    t!("modules.missing_dotnet_host"),
                ))
            })
            .when(rows.is_empty(), |section| {
                section.child(Notice::new(Tone::Problem, t!("modules.none_installed")))
            })
            .when(!rows.is_empty(), |section| {
                section.child(card(cx).flex().flex_col().children(rows))
            })
    }

    /// The registry: search it, and install from it.
    fn registry_card(&self, modules: &Modules, cx: &Context<Self>) -> impl IntoElement {
        let query = self.search.read(cx).value().to_string();
        let registry = &modules.registry;
        let rows = registry
            .modules
            .iter()
            .enumerate()
            .filter(|(_, entry)| matches(entry, &query))
            .enumerate()
            .map(|(shown, (index, entry))| {
                let installed = modules
                    .installed
                    .iter()
                    .find(|module| module.module_id.as_deref() == Some(&entry.module_id));
                // Rules go between the rows shown, whatever the search hides.
                self.registry_row(index, shown == 0, entry, installed, modules, cx)
            })
            .collect::<Vec<_>>();
        let refreshing = registry.loading || self.pending == Some(Pending::Refresh);
        let card = card(cx)
            .flex()
            .flex_col()
            .min_w_0()
            .child(
                v_flex()
                    .gap_3()
                    .p_4()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div().flex_1().min_w_0().child(
                                    Input::new(&self.search)
                                        .prefix(
                                            div()
                                                .text_color(palette::text_3())
                                                .child(Icon::new(IconName::Search).size(px(15.))),
                                        )
                                        .bg(palette::sunken()),
                                ),
                            )
                            .child(
                                icon_button(
                                    ("refresh-registry", 0),
                                    IconName::RefreshCw,
                                    t!("modules.fetch_again"),
                                )
                                .loading(refreshing)
                                .disabled(self.pending.is_some())
                                .on_click(cx.listener(|page, _, _, cx| page.refresh(cx))),
                            ),
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
            .when(!rows.is_empty(), |card| card.child(v_flex().children(rows)))
            .child(
                v_flex()
                    .gap_1()
                    .px_4()
                    .pt_3()
                    .pb_4()
                    .border_t_1()
                    .border_color(palette::line_soft())
                    .text_xs()
                    .text_color(palette::text_3())
                    .child(t!("modules.registry_footer"))
                    .when(!registry.url.is_empty(), |footer| {
                        footer.child(mono(registry.url.clone()).text_size(px(11.)).truncate())
                    }),
            );
        v_flex()
            .gap_3()
            .child(h_flex().gap_2().child(cap(t!("modules.get_modules"))).when(
                !registry.modules.is_empty(),
                |caption| {
                    caption.child(
                        div()
                            .font_family(MONO_FONT)
                            .text_size(px(11.))
                            .text_color(palette::text_4())
                            .child(registry.modules.len().to_string()),
                    )
                },
            ))
            .child(card)
    }
}

impl Render for ModulesPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let online = *self.daemon.read(cx).connection() == Connection::Online;
        let page = v_flex()
            .gap_6()
            .child(PageHeader::new(t!("modules.title")).description(t!("modules.description")));
        let add_ons = Section::new(t!("modules.add_ons")).child(self.switches.clone());
        if !online {
            // Switching an add-on still works: it writes config.json.
            return page
                .child(
                    v_flex()
                        .gap_3()
                        .items_start()
                        .child(hint(t!("modules.start_to_manage"), cx))
                        .child(StartVrft::new(self.launcher.clone())),
                )
                .child(add_ons)
                .into_any_element();
        }
        let Some(modules) = self.modules.clone() else {
            return page
                .child(hint(t!("modules.reading"), cx))
                .children(self.message.clone())
                .child(add_ons)
                .into_any_element();
        };
        let installed = v_flex()
            .flex_1()
            .min_w_0()
            .gap_6()
            .child(self.installed_section(&modules, cx))
            .child(add_ons);
        let registry = self.registry_card(&modules, cx);
        let columns = if vrft_gui_core::content_width(window) >= TWO_COLUMNS_MIN {
            h_flex()
                .items_start()
                .gap_6()
                .child(installed)
                .child(div().w(px(REGISTRY_WIDTH)).flex_none().child(registry))
                .into_any_element()
        } else {
            v_flex()
                .gap_6()
                .child(installed)
                .child(registry)
                .into_any_element()
        };
        page.children(self.message.clone())
            .child(columns)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_matches_name_author_and_description() {
        let entry = RegistryModule {
            module_name: "SteamLink VRCFT Module".into(),
            author_name: "Ykeara".into(),
            module_description: "Eye and face tracking for Steam Link".into(),
            ..RegistryModule::default()
        };
        assert!(matches(&entry, ""));
        assert!(matches(&entry, "steamlink"));
        assert!(matches(&entry, "ykeara"));
        assert!(matches(&entry, " FACE "));
        assert!(!matches(&entry, "varjo"));
    }

    #[test]
    fn vrcft_modules_need_the_dotnet_host() {
        let native = InstalledModule {
            runtime: "native".into(),
            ..InstalledModule::default()
        };
        let managed = InstalledModule {
            runtime: "dotnet".into(),
            ..InstalledModule::default()
        };
        let mut modules = Modules::default();
        assert!(unusable(&native, &modules).is_none());
        assert!(unusable(&managed, &modules).is_some());
        modules.dotnet_host = true;
        assert!(unusable(&managed, &modules).is_none());
    }

    #[test]
    fn busy_while_fetching_or_installing() {
        assert!(!busy(None));
        let mut modules = Modules::default();
        assert!(!busy(Some(&modules)));
        modules.registry.loading = true;
        assert!(busy(Some(&modules)));
        modules.registry.loading = false;
        modules.operations.push(ModuleOperation {
            state: OperationState::Installing,
            ..ModuleOperation::default()
        });
        assert!(busy(Some(&modules)));
        modules.operations[0].state = OperationState::Failed;
        assert!(!busy(Some(&modules)));
    }
}
