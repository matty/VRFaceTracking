//! Headset: reaches the Quest Pro with adb over USB or Wi-Fi, installs and
//! updates the VRFT headset app, and starts and stops its camera stream.
use crate::adb::{self, Adb, Details, Device, DeviceState, InstallError, Origin};
use crate::headset_app::{self, Comparison, Package, PackageOrigin};
use crate::live::DaemonState;
use crate::summary::Tone;
use crate::widgets::{info_row, Notice, PageHeader, Panel, StatusPill};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    h_flex, v_flex, ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, AnyElement, App, AppContext as _, ClipboardItem, Context, Entity,
    InteractiveElement as _, IntoElement, ParentElement, PathPromptOptions, Render, SharedString,
    StatefulInteractiveElement as _, Styled, Subscription, Task, Window,
};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Listing devices starts a process each time, so only while the page shows,
/// and not too often.
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const TICK: Duration = Duration::from_millis(250);
/// Below this content width the panels stack in one column.
const TWO_COLUMN_WIDTH: f32 = 760.;
const RELEASES_URL: &str = "https://github.com/matty/VRFaceTracking/releases";

enum AdbSetup {
    Looking,
    Missing,
    Found(Arc<Adb>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Busy {
    Downloading,
    Connecting,
    Disconnecting,
    Installing,
    Opening,
    Starting,
    Stopping,
    Restarting,
    ReadingLog,
}

/// How the page found the headset's Wi-Fi address without being told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FoundBy {
    /// The headset connected to adb reported it.
    Adb,
    /// VRFT's camera stream comes from it.
    Stream,
}

/// The panel an outcome is shown in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Area {
    Connection,
    App,
    Stream,
    Log,
}

/// What a finished action reports back.
#[derive(Default)]
struct Done {
    message: Option<String>,
    /// A device to act on from now on, such as a new Wi-Fi connection.
    select: Option<String>,
    log: Option<String>,
}

impl Done {
    fn message(message: impl Into<String>) -> Self {
        Self {
            message: Some(message.into()),
            ..Self::default()
        }
    }
}

/// One reading of adb's devices and the chosen device's details.
struct Snapshot {
    /// The device that was chosen when the reading started.
    requested: Option<String>,
    devices: Result<Vec<Device>, String>,
    selected: Option<String>,
    details: Option<Details>,
}

pub struct HeadsetPage {
    daemon: Entity<DaemonState>,
    app_dir: Option<PathBuf>,
    adb: AdbSetup,
    devices: Vec<Device>,
    /// Why adb couldn't list devices.
    devices_error: Option<String>,
    /// The device the page acts on.
    selected: Option<String>,
    details: Option<Details>,
    /// The headset app this VRFT offers.
    package: Option<Package>,
    busy: Option<Busy>,
    message: Option<(Area, Tone, SharedString)>,
    /// The package being installed, kept in case the headset's copy has to
    /// be removed first.
    installing: Option<Package>,
    /// A package that couldn't be installed over the headset's copy.
    replace: Option<Package>,
    log: Option<SharedString>,
    address: Entity<InputState>,
    /// The address last filled into the Wi-Fi field, which is only replaced
    /// while nobody has typed over it.
    suggested: Option<IpAddr>,
    watching: bool,
    refresh_requested: bool,
    _poll: Task<()>,
    _action: Option<Task<()>>,
    _subscriptions: [Subscription; 2],
}

impl HeadsetPage {
    pub fn new(daemon: Entity<DaemonState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let address = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Headset IP address, found automatically when it can be")
        });
        let subscriptions = [
            // The stream panel shows what VRFT receives.
            cx.observe(&daemon, |page, _, cx| {
                if page.watching {
                    cx.notify();
                }
            }),
            cx.subscribe(&address, |page, _, event: &InputEvent, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    page.connect(cx);
                }
            }),
        ];
        let poll = cx.spawn_in(window, async move |this, cx| {
            let mut last: Option<Instant> = None;
            loop {
                let Ok(next) = this.update(cx, |page, _| page.poll_target(last)) else {
                    break;
                };
                if let Some((adb, requested)) = next {
                    let snapshot = cx
                        .background_executor()
                        .spawn(async move { read_headset(&adb, requested) })
                        .await;
                    last = Some(Instant::now());
                    let applied =
                        this.update_in(cx, |page, window, cx| page.apply(snapshot, window, cx));
                    if applied.is_err() {
                        break;
                    }
                }
                cx.background_executor().timer(TICK).await;
            }
        });
        let mut page = Self {
            daemon,
            app_dir: crate::paths::app_dir(),
            adb: AdbSetup::Looking,
            devices: Vec::new(),
            devices_error: None,
            selected: None,
            details: None,
            package: None,
            busy: None,
            message: None,
            installing: None,
            replace: None,
            log: None,
            address,
            suggested: None,
            watching: false,
            refresh_requested: false,
            _poll: poll,
            _action: None,
            _subscriptions: subscriptions,
        };
        page.locate(cx);
        page
    }

    /// Looks for adb and the headset app package, off the UI thread.
    fn locate(&mut self, cx: &mut Context<Self>) {
        let app_dir = self.app_dir.clone();
        cx.spawn(async move |this, cx| {
            let (adb, package) = cx
                .background_executor()
                .spawn(async move {
                    let app_dir = app_dir.as_deref();
                    (Adb::locate(app_dir), app_dir.and_then(headset_app::find))
                })
                .await;
            this.update(cx, |page, cx| {
                page.adb = match adb {
                    Some(adb) => AdbSetup::Found(Arc::new(adb)),
                    None => AdbSetup::Missing,
                };
                if page.package.is_none() {
                    page.package = package;
                }
                page.refresh_requested = true;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Reads the headset only while the page shows.
    pub fn set_watching(&mut self, watching: bool, cx: &mut Context<Self>) {
        if self.watching == watching {
            return;
        }
        self.watching = watching;
        if watching {
            if matches!(self.adb, AdbSetup::Missing) {
                // adb may have been installed since.
                self.locate(cx);
            }
            self.refresh_requested = true;
        }
    }

    fn poll_target(&mut self, last: Option<Instant>) -> Option<(Arc<Adb>, Option<String>)> {
        let AdbSetup::Found(adb) = &self.adb else {
            return None;
        };
        let due = last.is_none_or(|at| at.elapsed() >= POLL_INTERVAL);
        if !self.watching || !(due || self.refresh_requested) {
            return None;
        }
        self.refresh_requested = false;
        Some((adb.clone(), self.selected.clone()))
    }

    fn apply(&mut self, snapshot: Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        match snapshot.devices {
            Ok(devices) => {
                self.devices = devices;
                self.devices_error = None;
            }
            Err(error) => {
                self.devices.clear();
                self.devices_error = Some(error);
            }
        }
        // Someone may have chosen another device while this was read.
        if self.selected == snapshot.requested {
            self.selected = snapshot.selected;
            self.details = snapshot.details;
        }
        self.suggest_address(window, cx);
        cx.notify();
    }

    /// The headset's address as found without asking: from the headset itself
    /// over adb, or from where VRFT's camera stream comes from.
    fn detected_address(&self, cx: &App) -> Option<(IpAddr, FoundBy)> {
        let from_usb = self
            .details
            .as_ref()
            .and_then(|details| details.wifi_address)
            .map(|ip| (IpAddr::V4(ip), FoundBy::Adb));
        from_usb.or_else(|| {
            self.daemon
                .read(cx)
                .status()
                .and_then(|status| status.source.as_deref())
                .and_then(|source| source.parse::<SocketAddr>().ok())
                .map(|address| address.ip())
                .filter(|ip| !ip.is_loopback())
                .map(|ip| (ip, FoundBy::Stream))
        })
    }

    /// Fills the Wi-Fi field with the detected address.
    fn suggest_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let suggestion = self.detected_address(cx).map(|(ip, _)| ip);
        if suggestion.is_none() || suggestion == self.suggested {
            return;
        }
        let typed = self.address.read(cx).value();
        let untouched = typed.is_empty()
            || self
                .suggested
                .is_some_and(|previous| typed == previous.to_string());
        if untouched {
            self.suggested = suggestion;
            if let Some(ip) = suggestion {
                self.address
                    .update(cx, |input, cx| input.set_value(ip.to_string(), window, cx));
            }
        }
    }

    /// Puts the detected address back after someone typed over it.
    fn use_detected(&mut self, ip: IpAddr, window: &mut Window, cx: &mut Context<Self>) {
        self.suggested = Some(ip);
        self.address
            .update(cx, |input, cx| input.set_value(ip.to_string(), window, cx));
        cx.notify();
    }

    /// The line under the Wi-Fi field, saying whether VRFT found the address
    /// by itself and how, so nobody has to wonder where it came from.
    fn address_hint(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let typed = self.address.read(cx).value();
        let Some((ip, by)) = self.detected_address(cx) else {
            return div()
                .text_xs()
                .text_color(muted)
                .child(
                    "VRFT fills this in by itself once the headset is plugged in with USB, or is streaming its cameras to VRFT. \
                     Otherwise, find the address on the headset under Settings > Wi-Fi, in your network's details.",
                )
                .into_any_element();
        };
        let how = match by {
            FoundBy::Adb => "the connected headset reported it",
            FoundBy::Stream => "VRFT is receiving the headset's camera stream from it",
        };
        if typed.trim() == ip.to_string() {
            return h_flex()
                .gap_1p5()
                .text_xs()
                .text_color(muted)
                .child(
                    Icon::new(IconName::CircleCheck)
                        .xsmall()
                        .text_color(theme.success),
                )
                .child(format!("Found automatically: {how}."))
                .into_any_element();
        }
        h_flex()
            .gap_2()
            .flex_wrap()
            .text_xs()
            .text_color(muted)
            .child(format!("VRFT found the headset at {ip}: {how}."))
            .child(
                Button::new("use-detected-address")
                    .ghost()
                    .xsmall()
                    .label(format!("Use {ip}"))
                    .on_click(
                        cx.listener(move |page, _, window, cx| page.use_detected(ip, window, cx)),
                    ),
            )
            .into_any_element()
    }

    fn selected_device(&self) -> Option<&Device> {
        let selected = self.selected.as_deref()?;
        self.devices.iter().find(|device| device.serial == selected)
    }

    /// The chosen device, if adb can use it.
    fn ready_serial(&self) -> Option<String> {
        self.selected_device()
            .filter(|device| device.is_ready())
            .map(|device| device.serial.clone())
    }

    /// Runs `work` with adb off the UI thread, one action at a time.
    fn run(
        &mut self,
        busy: Busy,
        area: Area,
        work: impl FnOnce(&Adb) -> anyhow::Result<Done> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let AdbSetup::Found(adb) = &self.adb else {
            return;
        };
        if self.busy.is_some() {
            return;
        }
        let adb = adb.clone();
        self.busy = Some(busy);
        self.message = None;
        cx.notify();
        self._action = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { work(&adb) })
                .await;
            this.update(cx, |page, cx| page.finish(area, result, cx))
                .ok();
        }));
    }

    fn finish(&mut self, area: Area, result: anyhow::Result<Done>, cx: &mut Context<Self>) {
        self.busy = None;
        let installing = self.installing.take();
        match result {
            Ok(done) => {
                if let Some(serial) = done.select {
                    self.selected = Some(serial);
                    self.details = None;
                }
                if let Some(log) = done.log {
                    self.log = Some(log.into());
                }
                self.message = done
                    .message
                    .map(|message| (area, Tone::Good, message.into()));
            }
            Err(error) => {
                if matches!(
                    error.downcast_ref::<InstallError>(),
                    Some(InstallError::DifferentSigner | InstallError::Downgrade)
                ) {
                    self.replace = installing;
                }
                self.message = Some((area, Tone::Problem, format!("{error:#}").into()));
            }
        }
        self.refresh_requested = true;
        cx.notify();
    }

    fn fail(&mut self, area: Area, message: &str, cx: &mut Context<Self>) {
        self.message = Some((area, Tone::Problem, message.to_string().into()));
        cx.notify();
    }

    fn select(&mut self, serial: String, cx: &mut Context<Self>) {
        self.selected = Some(serial);
        self.details = None;
        self.refresh_requested = true;
        cx.notify();
    }

    fn download_adb(&mut self, cx: &mut Context<Self>) {
        let Some(app_dir) = self.app_dir.clone() else {
            return;
        };
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(Busy::Downloading);
        self.message = None;
        cx.notify();
        self._action = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { adb::download_platform_tools(&app_dir) })
                .await;
            this.update(cx, |page, cx| {
                page.busy = None;
                match result {
                    Ok(adb) => {
                        let folder = adb
                            .path()
                            .parent()
                            .map(|dir| dir.display().to_string())
                            .unwrap_or_default();
                        page.message = Some((
                            Area::Connection,
                            Tone::Good,
                            format!("Downloaded adb into {folder}. VRFT uses it from now on.")
                                .into(),
                        ));
                        page.adb = AdbSetup::Found(Arc::new(adb));
                        page.refresh_requested = true;
                    }
                    Err(error) => {
                        page.message =
                            Some((Area::Connection, Tone::Problem, format!("{error:#}").into()));
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn use_wifi(&mut self, cx: &mut Context<Self>) {
        let (Some(serial), Some(ip)) = (
            self.ready_serial(),
            self.details
                .as_ref()
                .and_then(|details| details.wifi_address),
        ) else {
            return;
        };
        self.run(
            Busy::Connecting,
            Area::Connection,
            move |adb| {
                let serial = adb.enable_wireless(&serial, ip)?;
                Ok(Done {
                    select: Some(serial),
                    ..Done::message(format!(
                        "Connected over Wi-Fi at {ip}. You can unplug the cable; the headset takes adb over Wi-Fi until it restarts."
                    ))
                })
            },
            cx,
        );
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        let typed = self.address.read(cx).value();
        let Some(target) = adb::wireless_target(&typed) else {
            self.fail(
                Area::Connection,
                "Enter the headset's IP address, such as 192.168.1.20.",
                cx,
            );
            return;
        };
        self.run(
            Busy::Connecting,
            Area::Connection,
            move |adb| {
                let serial = adb.connect(&target)?;
                Ok(Done {
                    select: Some(serial),
                    ..Done::message(format!("Connected to {target}."))
                })
            },
            cx,
        );
    }

    fn disconnect(&mut self, serial: String, cx: &mut Context<Self>) {
        self.run(
            Busy::Disconnecting,
            Area::Connection,
            move |adb| {
                adb.disconnect(&serial)?;
                Ok(Done::default())
            },
            cx,
        );
    }

    /// Installs `package` over the headset's copy, or with `replace`, removes
    /// that copy first. A running stream is stopped first, so the app can put
    /// Meta's eye model back before Android replaces it.
    fn install(&mut self, package: Package, replace: bool, cx: &mut Context<Self>) {
        let Some(serial) = self.ready_serial() else {
            return;
        };
        if self.busy.is_some() {
            return;
        }
        self.replace = None;
        self.installing = Some(package.clone());
        self.run(
            Busy::Installing,
            Area::App,
            move |adb| {
                let was_streaming = adb.streaming(&serial)?;
                if was_streaming {
                    adb.stop_stream(&serial)?;
                }
                if replace {
                    adb.uninstall(&serial)?;
                }
                adb.install(&serial, &package.path)?;
                let mut message = format!("Installed headset app {}.", package.describe());
                if replace {
                    message
                        .push_str(" Magisk asks to allow root again when the stream next starts.");
                } else if was_streaming {
                    message.push_str(
                        " The stream stopped for the update; start it again when you're ready.",
                    );
                }
                Ok(Done::message(message))
            },
            cx,
        );
    }

    fn choose_package(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Install".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |page, cx| {
                let apk = path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("apk"));
                if apk {
                    page.install(Package::chosen(path), false, cx);
                } else {
                    page.fail(Area::App, "Choose an .apk file.", cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn open_app(&mut self, cx: &mut Context<Self>) {
        let Some(serial) = self.ready_serial() else {
            return;
        };
        self.run(
            Busy::Opening,
            Area::Stream,
            move |adb| {
                adb.open_app(&serial, &[])?;
                Ok(Done::default())
            },
            cx,
        );
    }

    fn start_stream(&mut self, cx: &mut Context<Self>) {
        let Some(serial) = self.ready_serial() else {
            return;
        };
        self.run(
            Busy::Starting,
            Area::Stream,
            move |adb| {
                adb.open_app(&serial, &[("start_probe", true)])?;
                Ok(Done::message(
                    "Starting. On the headset, allow root if Magisk asks, then go to Virtual Desktop.",
                ))
            },
            cx,
        );
    }

    fn stop_stream(&mut self, cx: &mut Context<Self>) {
        let Some(serial) = self.ready_serial() else {
            return;
        };
        self.run(
            Busy::Stopping,
            Area::Stream,
            move |adb| {
                adb.stop_stream(&serial)?;
                Ok(Done::message("Stopped."))
            },
            cx,
        );
    }

    /// Saves the headset app's eye gaze setting, which applies when its
    /// stream starts, so a running stream restarts.
    fn set_eye_gaze(&mut self, on: bool, cx: &mut Context<Self>) {
        let Some(serial) = self.ready_serial() else {
            return;
        };
        self.run(
            Busy::Restarting,
            Area::Stream,
            move |adb| {
                let streaming = adb.streaming(&serial)?;
                if streaming {
                    adb.stop_stream(&serial)?;
                }
                adb.open_app(&serial, &[("eye_enabled", on), ("start_probe", streaming)])?;
                let setting = if on { "on" } else { "off" };
                Ok(Done::message(if streaming {
                    format!("Independent eye gaze is {setting}, and the stream restarted. On the headset, go back to Virtual Desktop.")
                } else {
                    format!("Independent eye gaze will be {setting} when the stream starts.")
                }))
            },
            cx,
        );
    }

    fn read_log(&mut self, cx: &mut Context<Self>) {
        let Some(serial) = self.ready_serial() else {
            return;
        };
        self.run(
            Busy::ReadingLog,
            Area::Log,
            move |adb| {
                Ok(Done {
                    log: Some(adb.app_log(&serial)?),
                    ..Done::default()
                })
            },
            cx,
        );
    }

    /// The page's one-line state, for its header.
    fn state(&self) -> (Tone, SharedString) {
        match &self.adb {
            AdbSetup::Looking => return (Tone::Waiting, "Looking for adb".into()),
            AdbSetup::Missing => return (Tone::Problem, "adb not found".into()),
            AdbSetup::Found(_) => {}
        }
        if self.devices_error.is_some() {
            return (Tone::Problem, "adb isn't working".into());
        }
        let Some(device) = self.selected_device() else {
            return (Tone::Off, "No headset".into());
        };
        match &device.state {
            DeviceState::Ready if device.is_wireless() => {
                (Tone::Good, "Connected over Wi-Fi".into())
            }
            DeviceState::Ready => (Tone::Good, "Connected over USB".into()),
            DeviceState::Unauthorized => (Tone::Waiting, "Allow USB debugging".into()),
            DeviceState::Offline => (Tone::Problem, "Not responding".into()),
            DeviceState::Other(state) => (Tone::Waiting, state.clone().into()),
        }
    }

    fn message_in(&self, area: Area) -> Option<Notice> {
        self.message
            .as_ref()
            .filter(|(shown_in, _, _)| *shown_in == area)
            .map(|(_, tone, message)| Notice::new(*tone, message.clone()))
    }

    fn connection_panel(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let panel = Panel::new("Connection");
        let adb = match &self.adb {
            AdbSetup::Looking => {
                return panel
                    .child(div().text_sm().text_color(muted).child("Looking for adb…"))
                    .into_any_element()
            }
            AdbSetup::Missing => return self.missing_adb(panel, cx),
            AdbSetup::Found(adb) => adb.clone(),
        };
        let busy = self.busy.is_some();
        let several = self.devices.len() > 1;
        let wireless_ips: Vec<IpAddr> = self
            .devices
            .iter()
            .filter_map(|device| device.wireless_address().map(|address| address.ip()))
            .collect();
        let rows: Vec<AnyElement> = self
            .devices
            .iter()
            .enumerate()
            .map(|(index, device)| {
                let selected = self.selected.as_deref() == Some(device.serial.as_str());
                let wifi = selected
                    .then(|| {
                        self.details
                            .as_ref()
                            .and_then(|details| details.wifi_address)
                    })
                    .flatten()
                    .filter(|ip| !device.is_wireless() && !wireless_ips.contains(&IpAddr::V4(*ip)));
                self.device_row(index, device, selected, several, wifi.is_some(), cx)
            })
            .collect();
        let connecting = self.busy == Some(Busy::Connecting);
        let theme = cx.theme();
        panel
            .when_some(self.devices_error.clone(), |panel, error| {
                panel.child(Notice::new(Tone::Problem, error))
            })
            .when(self.devices.is_empty() && self.devices_error.is_none(), |panel| {
                panel.child(div().text_sm().text_color(theme.muted_foreground).child(
                    "No headset found. Connect the Quest Pro with a USB cable, with developer mode on, or connect over Wi-Fi below.",
                ))
            })
            .child(v_flex().gap_2().children(rows))
            .child(div().h(px(1.)).bg(theme.border))
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().font_medium().child("Connect over Wi-Fi"))
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .child(div().w(px(260.)).child(Input::new(&self.address)))
                            .child(
                                Button::new("connect-wifi")
                                    .outline()
                                    .icon(IconName::Wifi)
                                    .label("Connect")
                                    .loading(connecting)
                                    .disabled(busy)
                                    .on_click(cx.listener(|page, _, _, cx| page.connect(cx))),
                            ),
                    )
                    .child(self.address_hint(cx))
                    .child(div().text_xs().text_color(theme.muted_foreground).child(
                        "The headset takes adb over Wi-Fi once you choose Use Wi-Fi while it's on USB, until it restarts.",
                    )),
            )
            .children(self.message_in(Area::Connection))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .truncate()
                            .child(format!(
                                "adb {}: {}",
                                adb.origin().label(),
                                adb.path().display()
                            )),
                    )
                    // Offer VRFT its own copy while it borrows another's.
                    .when(
                        adb.origin() != Origin::App && self.app_dir.is_some(),
                        |row| {
                            row.child(
                                Button::new("download-adb-here")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Download)
                                    .label("Download to the VRFT folder")
                                    .tooltip(format!(
                                        "Downloads Google's Android SDK Platform-Tools into {}, keeps the files adb needs, and uses that adb from now on. Downloading them means accepting the Android SDK terms.",
                                        self.download_destination()
                                    ))
                                    .loading(self.busy == Some(Busy::Downloading))
                                    .disabled(busy)
                                    .on_click(cx.listener(|page, _, _, cx| page.download_adb(cx))),
                            )
                            .child(terms_button().xsmall())
                        },
                    ),
            )
            .into_any_element()
    }

    /// Where downloading adb puts it, for the page to say.
    fn download_destination(&self) -> String {
        self.app_dir
            .as_deref()
            .map(|dir| adb::platform_tools_dir(dir).display().to_string())
            .unwrap_or_else(|| "the VRFT folder".into())
    }

    fn device_row(
        &self,
        index: usize,
        device: &Device,
        selected: bool,
        several: bool,
        offer_wifi: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let busy = self.busy.is_some();
        let (tone, state) = match &device.state {
            DeviceState::Ready => (Tone::Good, "Connected".to_string()),
            DeviceState::Unauthorized => (
                Tone::Waiting,
                "Put on the headset and allow USB debugging for this computer".into(),
            ),
            DeviceState::Offline => (
                Tone::Problem,
                "Not responding. Unplug the headset and plug it back in".into(),
            ),
            DeviceState::Other(state) => (Tone::Waiting, state.clone()),
        };
        let link = if device.is_wireless() {
            format!("Wi-Fi · {}", device.serial)
        } else {
            format!("USB · {}", device.serial)
        };
        let serial = device.serial.clone();
        let disconnect_serial = device.serial.clone();
        h_flex()
            .gap_3()
            .p_3()
            .rounded(theme.radius)
            .border_1()
            .border_color(if selected && several {
                theme.primary.opacity(0.5)
            } else {
                theme.border
            })
            .child(
                Icon::new(if device.is_wireless() {
                    IconName::Wifi
                } else {
                    IconName::Usb
                })
                .text_color(theme.muted_foreground),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(div().text_sm().font_medium().child(device.name()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .truncate()
                            .child(link),
                    )
                    .child(Notice::new(tone, state)),
            )
            .when(several && !selected, |row| {
                row.child(
                    Button::new(("use-device", index))
                        .ghost()
                        .small()
                        .label("Use")
                        .disabled(busy)
                        .on_click(
                            cx.listener(move |page, _, _, cx| page.select(serial.clone(), cx)),
                        ),
                )
            })
            .when(offer_wifi, |row| {
                row.child(
                    Button::new(("use-wifi", index))
                        .outline()
                        .small()
                        .icon(IconName::Wifi)
                        .label("Use Wi-Fi")
                        .tooltip("Also connect over Wi-Fi, so the cable can come out")
                        .loading(self.busy == Some(Busy::Connecting))
                        .disabled(busy)
                        .on_click(cx.listener(|page, _, _, cx| page.use_wifi(cx))),
                )
            })
            .when(device.is_wireless(), |row| {
                row.child(
                    Button::new(("disconnect", index))
                        .ghost()
                        .small()
                        .label("Disconnect")
                        .loading(self.busy == Some(Busy::Disconnecting))
                        .disabled(busy)
                        .on_click(cx.listener(move |page, _, _, cx| {
                            page.disconnect(disconnect_serial.clone(), cx)
                        })),
                )
            })
            .into_any_element()
    }

    fn missing_adb(&self, panel: Panel, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let destination = self.download_destination();
        panel
            .child(div().text_sm().child(
                "VRFT reaches the headset through adb, Android's debug bridge, and didn't find it on this PC. \
                 It looks in the VRFT folder, for an adb that's already running, then on PATH and in the Android SDK, SideQuest and Meta Quest Developer Hub.",
            ))
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("download-adb")
                            .primary()
                            .icon(IconName::Download)
                            .label("Download adb")
                            .loading(self.busy == Some(Busy::Downloading))
                            .disabled(self.busy.is_some() || self.app_dir.is_none())
                            .on_click(cx.listener(|page, _, _, cx| page.download_adb(cx))),
                    )
                    .child(terms_button()),
            )
            .child(div().text_xs().text_color(theme.muted_foreground).child(format!(
                "Downloads Google's Android SDK Platform-Tools, about 8 MB, and keeps the files adb needs in {destination}. \
                 Downloading them means accepting the Android SDK terms. To use an adb somewhere else, set VRFT_ADB to its path."
            )))
            .children(self.message_in(Area::Connection))
            .into_any_element()
    }

    fn app_panel(&self, cx: &Context<Self>) -> AnyElement {
        let ready = self.ready_serial().is_some();
        let busy = self.busy.is_some();
        let details = self.details.as_ref().filter(|_| ready);
        let installed = details.and_then(|details| details.app.as_ref());
        let on_headset = match (details, installed) {
            (_, Some(app)) => app.version_name.clone(),
            (Some(_), None) => "Not installed".into(),
            (None, _) => "—".into(),
        };
        let offered = self.package.as_ref().map(|package| {
            let from = match package.origin {
                PackageOrigin::Bundled => "comes with VRFT",
                PackageOrigin::LocalBuild => "local build",
                PackageOrigin::Chosen => "chosen",
            };
            format!("{} ({from})", package.describe())
        });
        let install_button = self.package.clone().map(|package| {
            let comparison = headset_app::compare(installed, &package);
            let version = package.describe();
            let (label, primary) = match (details.is_some(), comparison) {
                (true, Comparison::NotInstalled) => (format!("Install {version}"), true),
                (true, Comparison::Update) => (format!("Update to {version}"), true),
                (true, Comparison::Same) => ("Reinstall".to_string(), false),
                _ => (format!("Install {version}"), false),
            };
            let button = Button::new("install")
                .icon(IconName::Package)
                .label(label)
                .loading(self.busy == Some(Busy::Installing))
                .disabled(!ready || busy)
                .on_click(
                    cx.listener(move |page, _, _, cx| page.install(package.clone(), false, cx)),
                );
            if primary {
                button.primary()
            } else {
                button.outline()
            }
        });
        let theme = cx.theme();
        let battery = details
            .and_then(|details| details.battery)
            .map(|battery| {
                if battery.charging {
                    format!("{}%, charging", battery.percent)
                } else {
                    format!("{}%", battery.percent)
                }
            })
            .unwrap_or_else(|| "—".into());
        Panel::new("Headset app")
            .child(
                v_flex()
                    .gap_2()
                    .child(info_row("On the headset", on_headset, cx))
                    .child(info_row(
                        "Available",
                        offered.clone().unwrap_or_else(|| "None".into()),
                        cx,
                    ))
                    .child(info_row("Battery", battery, cx))
                    .child(info_row(
                        "Firmware",
                        details
                            .and_then(|details| details.build.clone())
                            .unwrap_or_else(|| "—".into()),
                        cx,
                    )),
            )
            .when(offered.is_none(), |panel| {
                panel.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("This VRFT didn't come with the headset app. Download it from a headset app release, then choose Install from file."),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .children(install_button)
                    .child(
                        Button::new("install-file")
                            .ghost()
                            .icon(IconName::FolderOpen)
                            .label("Install from file…")
                            .disabled(!ready || busy)
                            .on_click(cx.listener(|page, _, _, cx| page.choose_package(cx))),
                    )
                    .when(offered.is_none(), |row| {
                        row.child(
                            Button::new("releases")
                                .ghost()
                                .icon(IconName::ExternalLink)
                                .label("Releases")
                                .on_click(|_, _, cx| cx.open_url(RELEASES_URL)),
                        )
                    }),
            )
            .when_some(self.replace.clone(), |panel, package| {
                panel.child(
                    v_flex()
                        .gap_2()
                        .items_start()
                        .child(div().text_sm().child(
                            "To install this one, the headset's copy has to be uninstalled first. That clears the headset app's settings, and Magisk asks to allow root again at the next start.",
                        ))
                        .child(
                            Button::new("replace")
                                .danger()
                                .label(format!("Uninstall and install {}", package.describe()))
                                .loading(self.busy == Some(Busy::Installing))
                                .disabled(!ready || busy)
                                .on_click(cx.listener(move |page, _, _, cx| {
                                    page.install(package.clone(), true, cx)
                                })),
                        ),
                )
            })
            .children(self.message_in(Area::App))
            .into_any_element()
    }

    fn stream_panel(&self, cx: &Context<Self>) -> AnyElement {
        let ready = self.ready_serial().is_some();
        let busy = self.busy.is_some();
        let details = self.details.as_ref().filter(|_| ready);
        let installed = details.is_some_and(|details| details.app.is_some());
        let streaming = details.is_some_and(|details| details.streaming);
        // The headset app reports its eye gaze setting only while VRFT
        // receives its stream.
        let eye_enabled = self
            .daemon
            .read(cx)
            .status()
            .filter(|status| status.source.is_some())
            .and_then(|status| status.headset.as_ref())
            .and_then(|headset| headset.eye.as_ref())
            .map(|eye| eye.enabled);
        let received = self
            .daemon
            .read(cx)
            .status()
            .is_some_and(|status| status.source.is_some());
        let theme = cx.theme();
        Panel::new("Camera stream")
            .child(
                v_flex()
                    .gap_2()
                    .child(info_row(
                        "On the headset",
                        match details {
                            Some(_) if streaming => "Running",
                            Some(_) => "Stopped",
                            None => "—",
                        },
                        cx,
                    ))
                    .child(info_row(
                        "VRFT",
                        if received { "Receiving it" } else { "Not receiving it" },
                        cx,
                    )),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(if streaming {
                        Button::new("stop-stream")
                            .outline()
                            .icon(IconName::CircleStop)
                            .label("Stop stream")
                            .loading(self.busy == Some(Busy::Stopping))
                            .disabled(!ready || busy)
                            .on_click(cx.listener(|page, _, _, cx| page.stop_stream(cx)))
                    } else {
                        Button::new("start-stream")
                            .primary()
                            .icon(IconName::Play)
                            .label("Start stream")
                            .loading(self.busy == Some(Busy::Starting))
                            .disabled(!ready || !installed || busy)
                            .on_click(cx.listener(|page, _, _, cx| page.start_stream(cx)))
                    })
                    .child(
                        Button::new("open-app")
                            .ghost()
                            .label("Open on headset")
                            .loading(self.busy == Some(Busy::Opening))
                            .disabled(!ready || !installed || busy)
                            .on_click(cx.listener(|page, _, _, cx| page.open_app(cx))),
                    ),
            )
            .when_some(eye_enabled, |panel, enabled| {
                panel.child(
                    Switch::new("headset-eye-gaze")
                        .label("Independent eye gaze (restarts the stream)")
                        .checked(enabled)
                        .disabled(!ready || busy)
                        .on_change(cx.listener(|page, checked: &bool, _, cx| {
                            page.set_eye_gaze(*checked, cx)
                        })),
                )
            })
            .child(div().text_xs().text_color(theme.muted_foreground).child(
                "Start and Stop open the headset app on the headset and press its buttons. The mouth cameras only run while Virtual Desktop streams.",
            ))
            .children(self.message_in(Area::Stream))
            .into_any_element()
    }

    fn log_panel(&self, cx: &Context<Self>) -> AnyElement {
        let ready = self.ready_serial().is_some();
        let busy = self.busy.is_some();
        let theme = cx.theme();
        let log = self.log.clone();
        Panel::new("Headset app log")
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("read-log")
                            .outline()
                            .small()
                            .icon(IconName::ScrollText)
                            .label(if log.is_some() { "Refresh" } else { "Show log" })
                            .loading(self.busy == Some(Busy::ReadingLog))
                            .disabled(!ready || busy)
                            .on_click(cx.listener(|page, _, _, cx| page.read_log(cx))),
                    )
                    .when_some(log.clone().filter(|log| !log.is_empty()), |row, log| {
                        row.child(
                            Button::new("copy-log")
                                .ghost()
                                .small()
                                .icon(IconName::Copy)
                                .label("Copy")
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        log.to_string(),
                                    ))
                                }),
                        )
                    }),
            )
            .when_some(log, |panel, log| {
                panel.child(if log.is_empty() {
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("The headset app hasn't logged anything recently.")
                        .into_any_element()
                } else {
                    div()
                        .id("headset-log")
                        .max_h(px(320.))
                        .overflow_y_scroll()
                        .p_3()
                        .rounded(theme.radius)
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.background)
                        .font_family(theme.mono_font_family.clone())
                        .text_xs()
                        .children(
                            log.lines()
                                .map(|line| div().child(line.to_string()))
                                .collect::<Vec<_>>(),
                        )
                        .into_any_element()
                })
            })
            .children(self.message_in(Area::Log))
            .into_any_element()
    }
}

impl Render for HeadsetPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (tone, state) = self.state();
        let two_columns = crate::shell::content_width(window) >= TWO_COLUMN_WIDTH;
        let connection = self.connection_panel(cx);
        let found = matches!(self.adb, AdbSetup::Found(_));
        v_flex()
            .gap_6()
            .child(
                PageHeader::new(
                    "Headset",
                    "Connect to the Quest Pro with adb, keep its VRFT app up to date, and start its camera stream.",
                )
                .trailing(StatusPill::new(tone, state)),
            )
            .child(connection)
            .when(found, |page| {
                page.child(
                    div()
                        .grid()
                        .grid_cols(if two_columns { 2 } else { 1 })
                        .gap_4()
                        .child(self.app_panel(cx))
                        .child(self.stream_panel(cx)),
                )
                .child(self.log_panel(cx))
            })
    }
}

/// Opens the terms that downloading Platform-Tools comes under.
fn terms_button() -> Button {
    Button::new("adb-terms")
        .ghost()
        .icon(IconName::ExternalLink)
        .label("Android SDK terms")
        .on_click(|_, _, cx| cx.open_url(adb::PLATFORM_TOOLS_TERMS_URL))
}

/// Lists adb's devices, keeps or picks the one to act on, and reads its
/// details.
fn read_headset(adb: &Adb, requested: Option<String>) -> Snapshot {
    let devices = match adb.devices() {
        Ok(devices) => devices,
        Err(error) => {
            return Snapshot {
                requested,
                devices: Err(format!("{error:#}")),
                selected: None,
                details: None,
            }
        }
    };
    let selected = choose(&devices, requested.as_deref());
    let details = selected
        .as_deref()
        .filter(|serial| {
            devices
                .iter()
                .any(|device| device.serial == *serial && device.is_ready())
        })
        .and_then(|serial| adb.details(serial).ok());
    Snapshot {
        requested,
        devices: Ok(devices),
        selected,
        details,
    }
}

/// The device to act on: the one already chosen while it's still there, or
/// else a ready Quest Pro, preferring USB.
fn choose(devices: &[Device], current: Option<&str>) -> Option<String> {
    if let Some(current) =
        current.filter(|current| devices.iter().any(|device| device.serial == *current))
    {
        return Some(current.to_string());
    }
    // `max_by_key` keeps the last of equals, so go backwards to keep adb's order.
    devices
        .iter()
        .rev()
        .max_by_key(|device| {
            (
                device.is_ready(),
                device.is_quest_pro(),
                !device.is_wireless(),
            )
        })
        .map(|device| device.serial.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_chosen_device_or_prefers_a_ready_quest_pro_on_usb() {
        let devices = adb::parse_devices(
            "List of devices attached
             emulator-5554          device product:sdk model:Pixel device:emu
             10.0.1.196:5555        device product:seacliff model:Quest_Pro device:seacliff
             230YC01D9K01SQ         device product:seacliff model:Quest_Pro device:seacliff
             1WMHH000000000         unauthorized usb:1-1
",
        );
        assert_eq!(choose(&devices, None).as_deref(), Some("230YC01D9K01SQ"));
        assert_eq!(
            choose(&devices, Some("10.0.1.196:5555")).as_deref(),
            Some("10.0.1.196:5555")
        );
        assert_eq!(
            choose(&devices, Some("gone")).as_deref(),
            Some("230YC01D9K01SQ")
        );
        assert_eq!(
            choose(&devices[3..], None).as_deref(),
            Some("1WMHH000000000")
        );
        assert_eq!(choose(&[], None), None);
    }
}
