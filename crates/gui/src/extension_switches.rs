//! Turning the extensions built into this app on and off, on the Modules
//! page. VRFT does it while it runs, restarting to apply it; while it
//! doesn't, `config.json` is written directly.
use crate::modules::icon_tile;
use crate::shell::Extensions;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{h_flex, v_flex, Disableable as _, Sizable as _, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, AnyElement, Context, Entity, IntoElement, ParentElement, Render, Styled, Subscription,
    Window,
};
use rust_i18n::t;
use vrft_gui_core::launcher::Launcher;
use vrft_gui_core::live::DaemonState;
use vrft_gui_core::palette;
use vrft_gui_core::summary::{Connection, Tone};
use vrft_gui_core::widgets::{card, hint, Notice, StatusDot};

pub struct ExtensionSwitches {
    daemon: Entity<DaemonState>,
    launcher: Entity<Launcher>,
    extensions: Entity<Extensions>,
    /// The extension being turned on or off.
    switching: Option<&'static str>,
    /// An extension asked to be turned on or off, waiting for "Restart now"
    /// or "Later".
    confirm_switch: Option<(&'static str, bool)>,
    /// How the last change went.
    outcome: Option<Notice>,
    _subscriptions: [Subscription; 3],
}

impl ExtensionSwitches {
    pub fn new(
        daemon: Entity<DaemonState>,
        launcher: Entity<Launcher>,
        extensions: Entity<Extensions>,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = [
            cx.observe(&daemon, |_, _, cx| cx.notify()),
            cx.observe(&launcher, |_, _, cx| cx.notify()),
            cx.observe(&extensions, |_, _, cx| cx.notify()),
        ];
        Self {
            daemon,
            launcher,
            extensions,
            switching: None,
            confirm_switch: None,
            outcome: None,
            _subscriptions: subscriptions,
        }
    }

    /// Turns extension `id` on or off: through VRFT while it runs, then
    /// restarting it if `restart`; straight in `config.json` while it
    /// doesn't.
    fn switch(&mut self, id: &'static str, enabled: bool, restart: bool, cx: &mut Context<Self>) {
        if self.switching.is_some() {
            return;
        }
        self.switching = Some(id);
        self.confirm_switch = None;
        self.outcome = None;
        cx.notify();
        let online = *self.daemon.read(cx).connection() == Connection::Online;
        let client = self.daemon.read(cx).client();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if online {
                        client.set_extension_enabled(id, enabled)
                    } else {
                        write_extension_enabled(id, enabled)
                    }
                })
                .await;
            this.update(cx, |switches, cx| {
                switches.switching = None;
                match result {
                    Ok(()) => {
                        switches
                            .extensions
                            .update(cx, |extensions, _| extensions.set_configured(id, enabled));
                        if online && restart {
                            switches
                                .launcher
                                .update(cx, |launcher, cx| launcher.restart(cx));
                        } else {
                            switches.outcome = Some(Notice::new(
                                Tone::Good,
                                t!("extension_switches.saved_next_start"),
                            ));
                        }
                    }
                    Err(error) => {
                        switches.outcome = Some(Notice::error(
                            &t!("extension_switches.change_failed"),
                            &error,
                        ));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn switch_pressed(&mut self, id: &'static str, enabled: bool, cx: &mut Context<Self>) {
        if *self.daemon.read(cx).connection() == Connection::Online {
            // Restarting stops tracking for a moment, so ask when.
            self.confirm_switch = Some((id, enabled));
            cx.notify();
        } else {
            self.switch(id, enabled, false, cx);
        }
    }

    /// One row per extension built into this app: what it adds, whether the
    /// daemon runs it, and a switch.
    fn rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let daemon = self.daemon.read(cx);
        let report = daemon.status().and_then(|status| status.daemon.as_ref());
        let busy = self.launcher.read(cx).is_busy() || self.switching.is_some();
        let extensions = self.extensions.read(cx);
        let mut rows = Vec::new();
        for extension in &extensions.list {
            let id = extension.id();
            // How it stands, and whether it can be switched from here.
            let (tone, state, enabled, reason) = match report.map(|report| report.extension(id)) {
                // VRFT doesn't answer yet; say what its config asks for.
                None => {
                    let enabled = extensions.configured(id);
                    let state = if enabled {
                        t!("extension_switches.on")
                    } else {
                        t!("extension_switches.off")
                    };
                    (Tone::Off, state, Some(enabled), None)
                }
                Some(None) => (
                    Tone::Off,
                    t!("extension_switches.not_built_in"),
                    None,
                    Some(t!("extension_switches.built_without").into_owned()),
                ),
                Some(Some(state)) => match (&state.error, state.enabled) {
                    (Some(error), _) => (
                        Tone::Problem,
                        t!("extension_switches.didnt_start"),
                        Some(true),
                        Some(error.clone()),
                    ),
                    (None, true) => (
                        Tone::Good,
                        t!("extension_switches.running"),
                        Some(true),
                        None,
                    ),
                    (None, false) => (Tone::Off, t!("extension_switches.off"), Some(false), None),
                },
            };
            let confirming = self.confirm_switch.filter(|(asked, _)| *asked == id);
            let switch = enabled.map(|enabled| {
                Switch::new(format!("extension-{id}"))
                    .checked(confirming.map_or(enabled, |(_, target)| target))
                    .disabled(busy)
                    .accessibility_label(t!("extension_switches.add_on", name = extension.name()))
                    .on_click(cx.listener(move |switches, checked: &bool, _, cx| {
                        switches.switch_pressed(id, *checked, cx)
                    }))
            });
            let confirm = confirming.map(|(_, target)| {
                // The buttons wrap under the text when the card is narrow.
                h_flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .pt_3()
                    .border_t_1()
                    .border_color(palette::line_soft())
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(200.))
                            .text_xs()
                            .text_color(palette::text_2())
                            .child(t!("extension_switches.restart_to_apply")),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .gap_2()
                            .child(
                                Button::new(format!("switch-later-{id}"))
                                    .ghost()
                                    .small()
                                    .label(t!("extension_switches.later"))
                                    .tooltip(t!("extension_switches.later_tooltip"))
                                    .on_click(cx.listener(move |switches, _, _, cx| {
                                        switches.switch(id, target, false, cx)
                                    })),
                            )
                            .child(
                                Button::new(format!("switch-now-{id}"))
                                    .primary()
                                    .small()
                                    .label(t!("extension_switches.restart_now"))
                                    .on_click(cx.listener(move |switches, _, _, cx| {
                                        switches.switch(id, target, true, cx)
                                    })),
                            ),
                    )
            });
            rows.push(
                card(cx)
                    .when(tone == Tone::Problem, |card| {
                        card.border_color(palette::signal_line())
                    })
                    .p_4()
                    .child(
                        v_flex()
                            .gap_3()
                            .child(
                                h_flex()
                                    .gap_3p5()
                                    // The same tile as a module's row above
                                    // it, with the icon in white.
                                    .child(
                                        icon_tile(extension.icon(), false, 36.)
                                            .text_color(palette::text()),
                                    )
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .gap_0p5()
                                            .child(
                                                h_flex()
                                                    .gap_2()
                                                    .text_sm()
                                                    .font_semibold()
                                                    .child(t!(
                                                        "extension_switches.add_on",
                                                        name = extension.name()
                                                    ))
                                                    .child(
                                                        h_flex()
                                                            .gap_1p5()
                                                            .text_xs()
                                                            .font_normal()
                                                            .text_color(palette::text_2())
                                                            .child(StatusDot::new(tone))
                                                            .child(state),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(palette::text_3())
                                                    .child(extension.description()),
                                            ),
                                    )
                                    .children(switch),
                            )
                            .children(reason.map(|reason| {
                                div()
                                    .text_xs()
                                    .text_color(if tone == Tone::Problem {
                                        palette::signal_text()
                                    } else {
                                        palette::text_3()
                                    })
                                    .child(reason)
                            }))
                            .children(confirm),
                    )
                    .into_any_element(),
            );
        }
        rows
    }
}

impl Render for ExtensionSwitches {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows(cx);
        v_flex()
            .gap_2p5()
            .children(rows)
            .children(self.outcome.clone())
            .child(
                div()
                    .max_w(px(560.))
                    .child(hint(t!("extension_switches.switching_hint"), cx)),
            )
    }
}

/// Turns an extension on or off straight in `config.json`, for while VRFT
/// isn't running to do it.
fn write_extension_enabled(id: &str, enabled: bool) -> anyhow::Result<()> {
    let path = vrft_gui_core::paths::config_file()
        .ok_or_else(|| anyhow::anyhow!("Can't find config.json"))?;
    let mut config: serde_json::Value = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(error) => return Err(error.into()),
    };
    vrft_protocol::set_extension_enabled(&mut config, id, enabled).map_err(anyhow::Error::msg)?;
    let pending = path.with_extension("json.pending");
    std::fs::write(&pending, serde_json::to_string_pretty(&config)?)?;
    std::fs::rename(&pending, &path)?;
    Ok(())
}
