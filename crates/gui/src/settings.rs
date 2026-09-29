//! Settings: where tracking is sent and how often, as saved in the daemon's
//! `config.json`, and the tracking module in use, which is changed on
//! Modules. Smoothing and the rest of the tuning are on Tracking settings.
//! Changes save as they're made, but apply when VRFT next starts, so a
//! banner offers to restart it or put back what it's running. Updates to
//! the app itself are here too.
use crate::modules::module_tile;
use crate::updates::{UpdateState, Updater};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{
    h_flex, v_flex, Disableable as _, Icon, Selectable as _, Sizable as _, StyledExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, AnyElement, AppContext as _, Context, Div, Entity, Hsla, IntoElement, ParentElement,
    Render, SharedString, Styled, Subscription, Task, Window,
};
use rust_i18n::t;
use std::time::Duration;
use vrft_gui_core::client::{Config, ConfigPatch, ModuleStatus, Plugin, RunMode, VrchatLink};
use vrft_gui_core::launcher::{Launcher, StartVrft};
use vrft_gui_core::live::DaemonState;
use vrft_gui_core::nav::{open_page, PageId};
use vrft_gui_core::palette::{self, MONO_FONT};
use vrft_gui_core::summary::{self, module_failure, module_state, Connection, Fix, Tone};
use vrft_gui_core::widgets::{
    card, fix_button, hint, ButtonExt as _, Notice, PageHeader, StatusDot, StatusLine,
};

const CHECK_INTERVAL: Duration = Duration::from_millis(250);
/// Where VRChat on this PC listens for OSC.
const VRCHAT_ADDRESS: &str = "127.0.0.1";
const VRCHAT_PORT: u16 = 9000;
/// Narrowest the content gets with each group's name beside its card; below
/// it they stack.
pub(crate) const GROUPS_WIDE_MIN: f32 = 720.;
/// The column naming each group.
const GROUP_LABEL_WIDTH: f32 = 250.;

/// The outputs the app offers. VRFaceTracking also has a generic UDP
/// output, but it's only for testing, so it's left to `config.json`.
const OFFERED_OUTPUTS: [&str; 1] = ["VRChat"];

/// The outputs VRFaceTracking has that the app offers, in its order.
fn offered_outputs(config: &Config) -> Vec<String> {
    config
        .output_modes
        .iter()
        .filter(|mode| OFFERED_OUTPUTS.contains(&mode.as_str()))
        .cloned()
        .collect()
}

/// "VRCFT module" or "Native module".
fn runtime_label(runtime: &str) -> SharedString {
    if runtime == "dotnet" {
        t!("settings.runtime_vrcft")
    } else {
        t!("settings.runtime_native")
    }
    .into()
}

/// Why `plugin` can't be chosen, when it can't.
fn unusable(plugin: &Plugin, config: &Config) -> Option<SharedString> {
    (plugin.runtime == "dotnet" && !config.dotnet_host)
        .then(|| t!("settings.needs_dotnet_host").into())
}

/// The settings that apply when VRFT restarts, as typed into the form.
#[derive(Debug, Clone, PartialEq)]
struct Form {
    output_mode: String,
    address: String,
    port: String,
    max_fps: String,
}

/// Which of the form's settings differ from a config.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct Changes {
    output_mode: bool,
    address: bool,
    port: bool,
    max_fps: bool,
}

impl Changes {
    fn any(self) -> bool {
        self.output_mode || self.address || self.port || self.max_fps
    }
}

impl Form {
    /// The form showing `config`.
    fn from_config(config: &Config) -> Self {
        Self {
            output_mode: config.output_mode.clone(),
            address: config.send_address.clone(),
            port: config.send_port.to_string(),
            max_fps: config
                .max_fps
                .map(|fps| format!("{fps}"))
                .unwrap_or_default(),
        }
    }

    /// Which settings saving the form would change in `config`. Text that
    /// isn't a number counts as a change, so saving says what's wrong with
    /// it.
    fn changes(&self, config: &Config) -> Changes {
        let saved = Self::from_config(config);
        let fps_differs = match self.max_fps() {
            Ok(fps) => {
                let limit = (fps > 0.).then_some(fps);
                match (limit, config.max_fps) {
                    (Some(a), Some(b)) => (a - b).abs() > 0.001,
                    (a, b) => a.is_some() != b.is_some(),
                }
            }
            Err(_) => true,
        };
        Changes {
            output_mode: self.output_mode != saved.output_mode,
            address: self.address.trim() != saved.address,
            port: self.port.trim() != saved.port,
            max_fps: fps_differs,
        }
    }

    /// Whether saving the form would change `config`.
    fn differs(&self, config: &Config) -> bool {
        self.changes(config).any()
    }

    /// The frame rate limit; 0 for none.
    fn max_fps(&self) -> Result<f32, String> {
        let fps = self.max_fps.trim();
        if fps.is_empty() {
            return Ok(0.);
        }
        fps.parse::<f32>()
            .ok()
            .filter(|fps| fps.is_finite() && *fps >= 0.)
            .ok_or_else(|| t!("settings.invalid_frame_rate", value = fps).into())
    }

    /// The form as a change to `config`, or why it can't be one. The module
    /// isn't part of it: choosing one saves it straight away.
    fn patch(&self, config: &Config) -> Result<ConfigPatch, String> {
        let port = self.port.trim();
        let port: u16 = port
            .parse()
            .ok()
            .filter(|port| *port > 0)
            .ok_or_else(|| t!("settings.invalid_port", value = port).to_string())?;
        Ok(ConfigPatch {
            module: None,
            // A mode the app doesn't offer (such as Resonite, or the generic
            // UDP output for testing, set by hand) is left as it is in the
            // file.
            output_mode: Some(self.output_mode.clone())
                .filter(|mode| offered_outputs(config).contains(mode)),
            send_address: Some(self.address.trim().to_string()),
            send_port: Some(port),
            max_fps: Some(self.max_fps()?),
            ..ConfigPatch::default()
        })
    }
}

pub struct SettingsPage {
    daemon: Entity<DaemonState>,
    launcher: Entity<Launcher>,
    updater: Entity<Updater>,
    /// The settings as last read from or saved to VRFT.
    config: Option<Config>,
    /// The settings VRFT is running with: as first read since it started.
    applied: Option<Config>,
    output_mode: String,
    address: Entity<InputState>,
    port: Entity<InputState>,
    max_fps: Entity<InputState>,
    /// Show the address while it's the default. VRChat is found by itself
    /// on this PC, so it's only needed for VRChat on another device.
    show_address: bool,
    /// The form at the last check; it saves once it's the same twice.
    settled: Option<Form>,
    /// A form that failed to save, so it isn't tried again.
    failed: Option<Form>,
    /// Why the form as typed can't be saved.
    invalid: Option<String>,
    /// Why reading or saving failed.
    message: Option<Notice>,
    /// What loading the module again did, or why it couldn't.
    module_message: Option<Notice>,
    saving: bool,
    /// The module being switched to, while VRFT is asked to.
    switching: Option<String>,
    watching: bool,
    /// Read the settings again at the next chance.
    stale: bool,
    _poll: Task<()>,
    _save: Option<Task<()>>,
    _switch: Option<Task<()>>,
    _subscriptions: [Subscription; 6],
}

impl SettingsPage {
    pub fn new(
        daemon: Entity<DaemonState>,
        launcher: Entity<Launcher>,
        updater: Entity<Updater>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let address = cx.new(|cx| InputState::new(window, cx).placeholder(VRCHAT_ADDRESS));
        let port = cx.new(|cx| InputState::new(window, cx).placeholder("9000"));
        let max_fps = cx.new(|cx| InputState::new(window, cx).placeholder(t!("settings.no_limit")));
        let client = daemon.read(cx).client();
        // Reads the settings while the page shows and they're stale, such as
        // after VRFT restarts.
        let poll = cx.spawn_in(window, async move |this, cx| loop {
            let Ok(read) = this.update(cx, |page, cx| {
                page.watching
                    && page.stale
                    && *page.daemon.read(cx).connection() == Connection::Online
            }) else {
                break;
            };
            if read {
                let client = client.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move { client.config() })
                    .await;
                if this
                    .update_in(cx, |page, window, cx| page.loaded(result, window, cx))
                    .is_err()
                {
                    break;
                }
            }
            if this.update(cx, |page, cx| page.autosave(cx)).is_err() {
                break;
            }
            cx.background_executor().timer(CHECK_INTERVAL).await;
        });
        let subscriptions = [
            cx.observe(&daemon, |page, daemon, cx| {
                // A restart applies what was saved; read it again.
                if *daemon.read(cx).connection() != Connection::Online {
                    page.stale = true;
                    page.applied = None;
                }
                cx.notify();
            }),
            cx.observe(&launcher, |_, _, cx| cx.notify()),
            cx.observe(&updater, |_, _, cx| cx.notify()),
            // Typing changes what's different from what VRFT runs.
            cx.observe(&address, |_, _, cx| cx.notify()),
            cx.observe(&port, |_, _, cx| cx.notify()),
            cx.observe(&max_fps, |_, _, cx| cx.notify()),
        ];
        Self {
            daemon,
            launcher,
            updater,
            config: None,
            applied: None,
            output_mode: String::new(),
            address,
            port,
            max_fps,
            show_address: false,
            settled: None,
            failed: None,
            invalid: None,
            message: None,
            module_message: None,
            saving: false,
            switching: None,
            watching: false,
            stale: true,
            _poll: poll,
            _save: None,
            _switch: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn set_watching(&mut self, watching: bool, cx: &mut Context<Self>) {
        self.watching = watching;
        if watching {
            self.stale = true;
        }
        cx.notify();
    }

    fn loaded(
        &mut self,
        result: anyhow::Result<Config>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stale = false;
        match result {
            Ok(config) => {
                if self.applied.is_none() {
                    self.applied = Some(config.clone());
                }
                // Typing that's yet to save isn't thrown away by a re-read.
                let unsaved = self
                    .config
                    .as_ref()
                    .is_some_and(|old| self.form(cx).differs(old));
                if unsaved {
                    self.config = Some(config);
                } else {
                    self.fill(config, window, cx);
                }
            }
            Err(error) => self.message = Some(Notice::error(&t!("settings.read_failed"), &error)),
        }
        cx.notify();
    }

    /// Shows `config` in the form.
    fn fill(&mut self, config: Config, window: &mut Window, cx: &mut Context<Self>) {
        self.show(Form::from_config(&config), window, cx);
        self.config = Some(config);
    }

    /// Puts `form` in the form's fields.
    fn show(&mut self, form: Form, window: &mut Window, cx: &mut Context<Self>) {
        self.output_mode = form.output_mode;
        let texts = [
            (&self.address, form.address),
            (&self.port, form.port),
            (&self.max_fps, form.max_fps),
        ];
        for (input, text) in texts {
            input.update(cx, |input, cx| input.set_value(text, window, cx));
        }
    }

    /// The form as typed.
    fn form(&self, cx: &Context<Self>) -> Form {
        Form {
            output_mode: self.output_mode.clone(),
            address: self.address.read(cx).value().to_string(),
            port: self.port.read(cx).value().to_string(),
            max_fps: self.max_fps.read(cx).value().to_string(),
        }
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

    fn extensions_only(&self, cx: &Context<Self>) -> bool {
        self.daemon
            .read(cx)
            .status()
            .and_then(|status| status.daemon.as_ref())
            .is_some_and(|daemon| daemon.mode == RunMode::ExtensionsOnly)
    }

    /// Saves `file` as the tracking module and has VRFT load it in place of
    /// the one running. `/status` then shows how loading goes.
    fn choose_module(&mut self, file: String, name: String, cx: &mut Context<Self>) {
        if self.switching.is_some() {
            return;
        }
        self.switching = Some(file.clone());
        self.module_message = None;
        cx.notify();
        let extensions_only = self.extensions_only(cx);
        let client = self.daemon.read(cx).client();
        self._switch = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { client.use_module(&file) })
                .await;
            this.update(cx, |page, cx| {
                page.switching = None;
                match result {
                    Ok(modules) => {
                        if let Some(config) = page.config.as_mut() {
                            config.module = modules.active;
                        }
                        page.module_message = extensions_only.then(|| {
                            Notice::new(Tone::Good, t!("settings.module_saved", name = name))
                        });
                    }
                    Err(error) => {
                        page.module_message =
                            Some(Notice::error(&t!("settings.switch_failed"), &error))
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Saves the form once it's stopped changing, so a slider or typing
    /// saves when it settles rather than at every step.
    fn autosave(&mut self, cx: &mut Context<Self>) {
        let Some(config) = self.config.as_ref() else {
            return;
        };
        if self.saving {
            return;
        }
        let form = self.form(cx);
        if self.settled.as_ref() != Some(&form) {
            self.settled = Some(form);
            return;
        }
        if !form.differs(config) || self.failed.as_ref() == Some(&form) {
            if self.invalid.take().is_some() {
                cx.notify();
            }
            return;
        }
        match form.patch(config) {
            Ok(_) => {
                self.invalid = None;
                self.save(false, cx);
            }
            Err(reason) => {
                if self.invalid.as_ref() != Some(&reason) {
                    self.invalid = Some(reason);
                    cx.notify();
                }
            }
        }
    }

    /// Puts back the settings VRFT is running with, which then save.
    fn revert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(applied) = self.applied.clone() {
            self.show(Form::from_config(&applied), window, cx);
        }
        self.failed = None;
        self.message = None;
        cx.notify();
    }

    /// Restarts VRFT so it uses what's saved, saving anything yet to save
    /// first.
    fn restart(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let unsaved = self
            .config
            .as_ref()
            .is_some_and(|config| self.form(cx).differs(config));
        if unsaved {
            self.save(true, cx);
        } else {
            self.launcher
                .update(cx, |launcher, cx| launcher.restart(cx));
        }
    }

    /// Saves the form, then restarts VRFT so it applies when `restart`.
    fn save(&mut self, restart: bool, cx: &mut Context<Self>) {
        let Some(config) = self.config.clone() else {
            return;
        };
        let form = self.form(cx);
        let patch = match form.patch(&config) {
            Ok(patch) => patch,
            Err(reason) => {
                self.invalid = Some(reason);
                cx.notify();
                return;
            }
        };
        self.saving = true;
        self.message = None;
        cx.notify();
        let client = self.daemon.read(cx).client();
        self._save = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { client.update_config(&patch) })
                .await;
            this.update(cx, |page, cx| {
                page.saving = false;
                match result {
                    Ok(config) => {
                        page.failed = None;
                        page.config = Some(config);
                        if restart {
                            page.stale = true;
                            page.launcher
                                .update(cx, |launcher, cx| launcher.restart(cx));
                        }
                    }
                    Err(error) => {
                        page.failed = Some(form);
                        page.message = Some(Notice::error(&t!("settings.save_failed"), &error));
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// The outputs VRFT offers, as one control: the chosen one raised.
    fn output_choice(&self, config: &Config, cx: &Context<Self>) -> impl IntoElement {
        h_flex()
            .gap(px(2.))
            .p(px(3.))
            .rounded(px(9.))
            .bg(palette::sunken())
            .border_1()
            .border_color(palette::line())
            .children(
                offered_outputs(config)
                    .into_iter()
                    .enumerate()
                    .map(|(index, mode)| {
                        let chosen = mode == self.output_mode;
                        Button::new(("output", index))
                            .ghost()
                            .small()
                            .h_7()
                            .px_4()
                            .rounded(px(6.))
                            .label(mode.clone())
                            .selected(chosen)
                            .map(|segment| {
                                if chosen {
                                    segment
                                        .bg(Hsla::from(gpui_kit::rgb(0x26262b)))
                                        .text_color(palette::text())
                                } else {
                                    segment.text_color(palette::text_2())
                                }
                            })
                            .on_click(cx.listener(move |page, _, _, cx| {
                                if let Some(mode) = page.config.as_ref().and_then(|config| {
                                    offered_outputs(config).into_iter().nth(index)
                                }) {
                                    page.output_mode = mode;
                                }
                                cx.notify();
                            }))
                    }),
            )
    }

    /// What OSCQuery has found of VRChat, for the running VRFT.
    fn vrchat_link(&self, cx: &Context<Self>) -> Option<VrchatLink> {
        self.daemon
            .read(cx)
            .status()?
            .daemon
            .as_ref()?
            .output
            .as_ref()?
            .vrchat
            .clone()
    }

    fn output_card(&self, config: &Config, changes: Changes, cx: &Context<Self>) -> Div {
        let vrchat = self.output_mode == "VRChat";
        let address = self.address.read(cx).value().trim().to_string();
        let port = self.port.read(cx).value().trim().to_string();
        // VRChat reports its port over OSCQuery, so the port only shows once
        // it has been changed by hand, or for another output.
        let port_shown = !vrchat || port != VRCHAT_PORT.to_string();
        let custom = address != VRCHAT_ADDRESS || port != VRCHAT_PORT.to_string();
        let open = self.show_address || custom || !vrchat;
        let address_field = v_flex()
            .flex_1()
            .min_w(px(140.))
            .gap_1p5()
            .child(field_label(t!("settings.address"), changes.address))
            .child(field(&self.address, changes.address));
        card(cx)
            .p(px(18.))
            .flex()
            .flex_col()
            .gap_4()
            .when(!offered_outputs(config).is_empty(), |card| {
                card.child(h_flex().child(self.output_choice(config, cx)))
            })
            .when(vrchat, |card| {
                card.child(discovery(self.vrchat_link(cx).as_ref(), cx))
            })
            .when(!open, |card| {
                card.child(
                    h_flex().child(
                        Button::new("show-address")
                            .ghost()
                            .small()
                            .h_7()
                            .ml(px(-8.))
                            .icon(IconName::ChevronRight)
                            .label(t!("settings.another_device"))
                            .on_click(cx.listener(|page, _, _, cx| {
                                page.show_address = true;
                                cx.notify();
                            })),
                    ),
                )
            })
            .when(open, |card| {
                card.child(
                    v_flex()
                        .gap_2()
                        .child(h_flex().gap_3().items_end().child(address_field).when(
                            port_shown,
                            |row| {
                                row.child(
                                    v_flex()
                                        .w(px(120.))
                                        .flex_none()
                                        .gap_1p5()
                                        .child(field_label(t!("settings.port"), changes.port))
                                        .child(field(&self.port, changes.port)),
                                )
                            },
                        ))
                        .when(vrchat, |fields| {
                            fields.child(hint(t!("settings.another_device_hint"), cx))
                        }),
                )
            })
            .when(custom, |card| {
                card.child(
                    h_flex().child(
                        Button::new("vrchat-defaults")
                            .ghost()
                            .small()
                            .h_7()
                            .ml(px(-8.))
                            .icon(IconName::RotateCcw)
                            .label(t!("settings.use_this_pc"))
                            .on_click(cx.listener(|page, _, window, cx| {
                                page.address.update(cx, |input, cx| {
                                    input.set_value(VRCHAT_ADDRESS, window, cx)
                                });
                                page.port.update(cx, |input, cx| {
                                    input.set_value(VRCHAT_PORT.to_string(), window, cx)
                                });
                                page.show_address = false;
                                cx.notify();
                            })),
                    ),
                )
            })
    }

    fn motion_card(&self, changes: Changes, cx: &Context<Self>) -> Div {
        card(cx).p(px(18.)).flex().flex_col().gap_4().child(
            h_flex()
                .gap_3()
                .items_center()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_0p5()
                        .child(
                            h_flex()
                                .gap_1p5()
                                .text_size(px(13.5))
                                .font_medium()
                                .child(t!("settings.frame_rate_limit"))
                                .when(changes.max_fps, |label| label.child(changed_mark())),
                        )
                        .child(hint(t!("settings.frame_rate_limit_hint"), cx)),
                )
                .child(
                    div()
                        .w(px(110.))
                        .flex_none()
                        .child(field(&self.max_fps, changes.max_fps)),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(12.5))
                        .text_color(palette::text_3())
                        .child(t!("settings.per_second")),
                ),
        )
    }

    /// The module in use, with the way to Modules to change it, and
    /// anything wrong with it under it.
    fn module_card(&self, config: &Config, cx: &Context<Self>) -> Div {
        let status = self.module_status(cx);
        let plugin = config
            .modules
            .iter()
            .find(|plugin| plugin.file == config.module);
        let installed = plugin.is_some();
        let chosen = !config.module.is_empty();
        let blocked = plugin.and_then(|plugin| unusable(plugin, config));
        let failure = module_failure(status.as_ref()).filter(|_| self.switching.is_none());
        let (tone, state) = if self.switching.is_some() {
            (Tone::Waiting, t!("settings.switching").to_string())
        } else if let Some(plugin) = plugin {
            let (tone, label) = module_state(status.as_ref());
            (
                tone,
                t!(
                    "settings.module_state",
                    runtime = runtime_label(&plugin.runtime),
                    state = label
                )
                .to_string(),
            )
        } else if chosen {
            (Tone::Problem, t!("settings.not_in_plugins").to_string())
        } else {
            (Tone::Off, t!("settings.choose_on_modules").to_string())
        };
        let name = match plugin {
            Some(plugin) => plugin.name.clone(),
            None if chosen => config.module.clone(),
            None => t!("settings.no_module_chosen").into(),
        };

        let mut notices: Vec<AnyElement> = Vec::new();
        if config.modules.is_empty() {
            notices.push(Notice::new(Tone::Problem, t!("settings.no_modules")).into_any_element());
        }
        if !installed && chosen {
            notices.push(
                Notice::new(
                    Tone::Problem,
                    t!("settings.chosen_missing", module = config.module),
                )
                .into_any_element(),
            );
        }
        if blocked.is_some() {
            notices.push(
                Notice::new(Tone::Problem, t!("settings.dotnet_host_missing")).into_any_element(),
            );
        }
        if let Some(error) = failure.filter(|_| installed) {
            let file = config.module.clone();
            let retry_name = name.clone();
            notices.push(
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Notice::new(Tone::Problem, error)),
                    )
                    .child(
                        Button::new("retry-module")
                            .regular()
                            .icon(IconName::RotateCcw)
                            .label(t!("settings.try_again"))
                            .tooltip(
                                blocked
                                    .clone()
                                    .unwrap_or_else(|| t!("settings.load_again").into()),
                            )
                            .loading(self.switching.is_some())
                            .disabled(self.switching.is_some() || blocked.is_some())
                            .on_click(cx.listener(move |page, _, _, cx| {
                                page.choose_module(file.clone(), retry_name.clone(), cx)
                            })),
                    )
                    .into_any_element(),
            );
        }
        notices.extend(
            self.module_message
                .clone()
                .map(IntoElement::into_any_element),
        );

        card(cx)
            .px(px(18.))
            .py(px(14.))
            .flex()
            .flex_col()
            .gap_3()
            .child(
                h_flex()
                    .gap_3p5()
                    .child(module_tile(tone == Tone::Good, 32.))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(1.))
                            .child(
                                div()
                                    .text_size(px(13.5))
                                    .font_semibold()
                                    .truncate()
                                    .child(name),
                            )
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .min_w_0()
                                    .text_xs()
                                    .text_color(if tone == Tone::Problem {
                                        palette::signal_text()
                                    } else {
                                        palette::text_3()
                                    })
                                    .when(matches!(tone, Tone::Problem | Tone::Waiting), |line| {
                                        line.child(StatusDot::new(tone))
                                    })
                                    .child(div().min_w_0().truncate().child(state)),
                            ),
                    )
                    .child(
                        Button::new("change-module")
                            .small()
                            .h_7()
                            .px_2p5()
                            .label(t!("settings.change"))
                            .child(Icon::new(IconName::ChevronRight).size(px(13.)))
                            .tooltip(t!("settings.change_tooltip"))
                            .on_click(|_, _, cx| open_page(PageId::MODULES, cx)),
                    ),
            )
            .children(notices)
    }
}

/// How VRChat is found: over OSCQuery, which gives its port, and what it
/// has found so far.
fn discovery<T: 'static>(link: Option<&VrchatLink>, cx: &Context<T>) -> impl IntoElement {
    let (tone, state) = match link {
        None => (Tone::Off, t!("settings.discovery_not_running")),
        Some(link) if !link.found => (Tone::Waiting, t!("settings.discovery_looking")),
        Some(link) => (
            match link.avatar_face_tracking {
                Some(false) => Tone::Waiting,
                _ => Tone::Good,
            },
            match link.avatar_face_tracking {
                None => t!("settings.discovery_found", address = link.sending_to),
                Some(true) => t!(
                    "settings.discovery_face_tracking",
                    address = link.sending_to
                ),
                Some(false) => {
                    t!(
                        "settings.discovery_no_face_tracking",
                        address = link.sending_to
                    )
                }
            },
        ),
    };
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .gap_2()
                .text_size(px(13.))
                .child(StatusDot::new(tone))
                .child(state),
        )
        .child(hint(t!("settings.discovery_how"), cx))
}

/// This build's version and how updating it stands, with the button to
/// check again or to restart into a downloaded update.
fn updates_card(updater: &Entity<Updater>, cx: &Context<SettingsPage>) -> Div {
    let state = updater.read(cx).state().clone();
    let channel = if crate::updates::is_dev() {
        t!("updates.channel_dev")
    } else {
        t!("updates.channel_stable")
    };
    let (tone, status) = match &state {
        UpdateState::Unavailable => (Tone::Off, t!("updates.unavailable")),
        UpdateState::Idle => (Tone::Off, t!("updates.idle")),
        UpdateState::Checking => (Tone::Waiting, t!("updates.checking")),
        UpdateState::UpToDate => (Tone::Good, t!("updates.up_to_date")),
        UpdateState::Downloading(version) => {
            (Tone::Waiting, t!("updates.downloading", version = version))
        }
        UpdateState::Ready(version) => (Tone::Good, t!("updates.ready", version = version)),
        UpdateState::Failed(error) => (Tone::Problem, t!("updates.failed", error = error)),
    };
    let busy = matches!(state, UpdateState::Checking | UpdateState::Downloading(_));
    let button = match state {
        UpdateState::Unavailable => None,
        UpdateState::Ready(_) => Some(
            Button::new("restart-to-update")
                .primary()
                .regular()
                .icon(IconName::RotateCcw)
                .label(t!("updates.restart"))
                .tooltip(t!("updates.restart_tooltip"))
                .on_click({
                    let updater = updater.clone();
                    move |_, window, cx| {
                        updater.update(cx, |updater, cx| updater.restart_to_update(window, cx))
                    }
                }),
        ),
        _ => Some(
            Button::new("check-for-updates")
                .ghost()
                .regular()
                .label(t!("updates.check"))
                .loading(busy)
                .disabled(busy)
                .on_click({
                    let updater = updater.clone();
                    move |_, _, cx| updater.update(cx, |updater, cx| updater.check(cx))
                }),
        ),
    };
    card(cx).p(px(18.)).child(
        h_flex()
            .gap_3()
            .items_center()
            .flex_wrap()
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(220.))
                    .gap_1p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_baseline()
                            .child(
                                div().text_size(px(13.5)).font_medium().child(t!(
                                    "updates.version",
                                    version = crate::updates::VERSION
                                )),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(palette::text_3())
                                    .child(channel),
                            ),
                    )
                    .child(StatusLine::new(tone, status)),
            )
            .children(button),
    )
}

/// Says that saved changes wait for a restart, with buttons to restart now
/// or put back what VRFT is running with. `revert` and `restart` are the
/// page's.
pub(crate) fn restart_banner<T: 'static>(
    busy: bool,
    restarting: bool,
    cx: &Context<T>,
    revert: fn(&mut T, &mut Window, &mut Context<T>),
    restart: fn(&mut T, &mut Window, &mut Context<T>),
) -> impl IntoElement {
    h_flex()
        .gap_3()
        .items_center()
        .flex_wrap()
        .pl(px(16.))
        .pr_3()
        .py_2p5()
        .rounded(px(12.))
        .border_1()
        .border_color(palette::line_strong())
        .bg(palette::inset())
        .child(
            h_flex()
                .flex_1()
                .min_w(px(240.))
                .gap_3()
                .child(
                    Icon::new(IconName::RotateCcw)
                        .size(px(15.))
                        .text_color(palette::text_2()),
                )
                .child(
                    v_flex()
                        .min_w_0()
                        .gap(px(1.))
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_medium()
                                .child(t!("settings.restart_to_apply")),
                        )
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(palette::text_3())
                                .child(t!("settings.restart_to_apply_detail")),
                        ),
                ),
        )
        .child(
            h_flex()
                .gap_2()
                .flex_none()
                .child(
                    Button::new("revert")
                        .ghost()
                        .regular()
                        .label(t!("settings.revert"))
                        // No tooltips while restarting: one begun on the way
                        // to Restart now would otherwise pop up as the
                        // banner changes under the pointer.
                        .when(!busy, |button| {
                            button.tooltip(t!("settings.revert_tooltip"))
                        })
                        .disabled(busy)
                        .on_click(cx.listener(move |page, _, window, cx| revert(page, window, cx))),
                )
                .child(
                    Button::new("restart-now")
                        .primary()
                        .regular()
                        .icon(IconName::RotateCcw)
                        .label(t!("settings.restart_now"))
                        .when(!busy, |button| {
                            button.tooltip(t!("settings.restart_now_tooltip"))
                        })
                        .loading(restarting)
                        .disabled(busy)
                        .on_click(
                            cx.listener(move |page, _, window, cx| restart(page, window, cx)),
                        ),
                ),
        )
}

/// A small white dot beside a setting's name: it differs from what VRFT is
/// running with, so it applies at the next restart.
pub(crate) fn changed_mark() -> Div {
    div()
        .flex_none()
        .size(px(6.))
        .rounded_full()
        .bg(palette::text())
}

/// A text field's name above it, marked when it's changed.
pub(crate) fn field_label(label: impl Into<SharedString>, changed: bool) -> impl IntoElement {
    h_flex()
        .gap_1p5()
        .text_size(px(12.5))
        .text_color(palette::text_2())
        .child(label.into())
        .when(changed, |label| label.child(changed_mark()))
}

/// A text field for a number or address, edged brighter when changed.
pub(crate) fn field(state: &Entity<InputState>, changed: bool) -> Input {
    Input::new(state)
        .font_family(MONO_FONT)
        .bg(palette::sunken())
        .when(changed, |input| input.border_color(palette::line_focus()))
}

/// One group of settings: its name and what it's for on the left, its card
/// on the right, or stacked when there isn't room.
pub(crate) fn group(
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    body: impl IntoElement,
    wide: bool,
) -> AnyElement {
    let about = v_flex()
        .gap_1p5()
        .child(
            div()
                .text_size(px(14.5))
                .line_height(px(20.))
                .font_semibold()
                .text_color(palette::text())
                .child(title.into()),
        )
        .child(
            div()
                .text_size(px(12.5))
                .line_height(px(18.))
                .text_color(palette::text_3())
                .child(description.into()),
        );
    if wide {
        h_flex()
            .items_start()
            .gap_8()
            .child(about.w(px(GROUP_LABEL_WIDTH)).flex_none().pt_1())
            .child(div().flex_1().min_w_0().child(body))
            .into_any_element()
    } else {
        v_flex().gap_3().child(about).child(body).into_any_element()
    }
}

impl Render for SettingsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.daemon.read(cx);
        let online = *state.connection() == Connection::Online;
        let config_error = state
            .status()
            .and_then(|status| status.daemon.as_ref())
            .and_then(|daemon| daemon.config_error.clone());
        let header = PageHeader::new(t!("settings.title"))
            .description(t!("settings.description"))
            .trailing(
                fix_button("open-config", Fix::Config)
                    .ghost()
                    .regular()
                    .icon(IconName::FileText),
            );
        let page = v_flex()
            .gap_6()
            .child(header)
            .when_some(config_error, |page, error| {
                page.child(Notice::new(Tone::Problem, summary::config_error(&error)))
            });
        let wide = vrft_gui_core::content_width(window) >= GROUPS_WIDE_MIN;
        // Updating the app doesn't need tracking running.
        let updates = group(
            t!("updates.title"),
            if crate::updates::is_dev() {
                t!("updates.description_dev")
            } else {
                t!("updates.description")
            },
            updates_card(&self.updater, cx),
            wide,
        );
        if !online {
            return page
                .child(
                    v_flex()
                        .gap_3()
                        .items_start()
                        .child(hint(t!("settings.start_to_change"), cx))
                        .child(StartVrft::new(self.launcher.clone())),
                )
                .child(updates)
                .into_any_element();
        }
        let Some(config) = self.config.clone() else {
            return page
                .child(hint(t!("settings.reading"), cx))
                .children(self.message.clone())
                .child(updates)
                .into_any_element();
        };
        let applied = self.applied.clone().unwrap_or_else(|| config.clone());
        let changes = self.form(cx).changes(&applied);
        let pending = Form::from_config(&config).differs(&applied);
        let restarting = self.launcher.read(cx).is_busy();
        page.when(pending, |page| {
            page.child(restart_banner(
                self.saving || restarting,
                restarting,
                cx,
                Self::revert,
                Self::restart,
            ))
        })
        .children(
            self.invalid
                .clone()
                .map(|reason| Notice::new(Tone::Problem, reason)),
        )
        .children(self.message.clone())
        .child(
            v_flex()
                .gap_6()
                .child(group(
                    t!("settings.send_to"),
                    t!("settings.send_to_description"),
                    self.output_card(&config, changes, cx),
                    wide,
                ))
                .child(group(
                    t!("settings.frame_rate"),
                    t!("settings.frame_rate_description"),
                    self.motion_card(changes, cx),
                    wide,
                ))
                .child(group(
                    t!("settings.tracking_module"),
                    t!("settings.tracking_module_description"),
                    self.module_card(&config, cx),
                    wide,
                ))
                .child(updates),
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config {
            output_mode: "VRChat".into(),
            output_modes: vec!["VRChat".into(), "Generic".into()],
            send_address: "127.0.0.1".into(),
            send_port: 9000,
            max_fps: Some(60.),
            ..Config::default()
        }
    }

    #[test]
    fn only_vrchat_is_offered() {
        assert_eq!(offered_outputs(&config()), vec!["VRChat".to_string()]);
    }

    #[test]
    fn the_patch_leaves_a_generic_output_set_by_hand() {
        let config = Config {
            output_mode: "Generic".into(),
            ..config()
        };
        let patch = Form::from_config(&config).patch(&config).unwrap();
        assert_eq!(patch.output_mode, None);
    }

    #[test]
    fn an_untouched_form_has_nothing_to_save() {
        let config = config();
        let form = Form::from_config(&config);
        assert!(!form.differs(&config));
        // Spacing and an equal number written differently aren't changes.
        let spaced = Form {
            address: " 127.0.0.1 ".into(),
            max_fps: "60.0".into(),
            ..form.clone()
        };
        assert!(!spaced.differs(&config));
        let no_limit = Config {
            max_fps: None,
            ..config.clone()
        };
        let zero = Form {
            max_fps: "0".into(),
            ..Form::from_config(&no_limit)
        };
        assert!(!zero.differs(&no_limit));
    }

    #[test]
    fn edits_are_changes() {
        let config = config();
        let form = Form::from_config(&config);
        for changed in [
            Form {
                port: "9001".into(),
                ..form.clone()
            },
            Form {
                max_fps: "".into(),
                ..form.clone()
            },
            Form {
                max_fps: "fast".into(),
                ..form.clone()
            },
            Form {
                output_mode: "Generic".into(),
                ..form.clone()
            },
        ] {
            assert!(changed.differs(&config), "{changed:?}");
        }
    }

    #[test]
    fn changes_name_the_field_that_changed() {
        let config = config();
        let form = Form::from_config(&config);
        assert_eq!(form.changes(&config), Changes::default());
        let port = Form {
            port: "9001".into(),
            ..form.clone()
        };
        assert_eq!(
            port.changes(&config),
            Changes {
                port: true,
                ..Changes::default()
            }
        );
        let limit = Form {
            max_fps: "fast".into(),
            ..form.clone()
        };
        assert_eq!(
            limit.changes(&config),
            Changes {
                max_fps: true,
                ..Changes::default()
            }
        );
    }

    #[test]
    fn the_patch_leaves_the_module_and_unoffered_outputs_alone() {
        let config = Config {
            output_mode: "Resonite".into(),
            ..config()
        };
        let patch = Form::from_config(&config).patch(&config).unwrap();
        assert_eq!(patch.module, None);
        assert_eq!(patch.output_mode, None);
        assert_eq!(patch.max_fps, Some(60.));
        assert_eq!(
            patch.smoothing, None,
            "smoothing is saved on Tracking settings"
        );
        let bad = Form {
            port: "0".into(),
            ..Form::from_config(&config)
        };
        assert!(bad.patch(&config).unwrap_err().contains("port"));
    }

    #[test]
    fn vrcft_modules_need_the_dotnet_host() {
        let managed = Plugin {
            runtime: "dotnet".into(),
            ..Plugin::default()
        };
        let native = Plugin {
            runtime: "native".into(),
            ..Plugin::default()
        };
        let mut config = config();
        assert!(unusable(&managed, &config).is_some());
        assert!(unusable(&native, &config).is_none());
        config.dotnet_host = true;
        assert!(unusable(&managed, &config).is_none());
    }
}
