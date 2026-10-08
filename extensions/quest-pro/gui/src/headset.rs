//! Headset: reaches the Quest Pro with adb over USB or Wi-Fi, installs and
//! updates the VRFT headset app, and starts and stops its camera stream.
use crate::adb::{self, Adb, Details, Device, DeviceState, InstallError, NotAllowed, Origin};
use crate::daemon::Update;
use crate::headset_app::{self, Comparison, Package, PackageOrigin};
use crate::live::QuestProState;
use crate::summary::Tone;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex, Disableable as _, Icon, Sizable as _, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    canvas, div, point, px, rgb, AnyElement, App, AppContext as _, Bounds, ClipboardItem, Context,
    Div, Entity, Hsla, InteractiveElement as _, IntoElement, ParentElement, PathBuilder,
    PathPromptOptions, Pixels, Render, SharedString, Stateful, StatefulInteractiveElement as _,
    Styled, Subscription, Task, Window,
};
use rust_i18n::t;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use vrft_gui_core::palette::{self, MONO_FONT};
use vrft_gui_core::widgets::{
    card, mono, ButtonExt as _, Notice, PageHeader, Panel, StatusDot, StatusLine,
};

/// Listing devices starts a process each time, so only while the page shows,
/// and not too often.
const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// A headset on Wi-Fi that hasn't allowed this computer is asked again this
/// often: once Allow is chosen, it takes the next request.
const ASK_AGAIN_AFTER: Duration = Duration::from_secs(10);
const TICK: Duration = Duration::from_millis(250);
/// How long a success notice stays.
const SUCCESS_SHOWN_FOR: Duration = Duration::from_secs(15);

/// How long a started stream gets to reach VRFT before the page says what's
/// wrong.
const STREAM_GRACE: Duration = Duration::from_secs(20);

/// Below this content width the columns stack.
const TWO_COLUMN_WIDTH: f32 = 760.;
/// The right-hand column: the headset app, per-eye gaze and Wi-Fi.
const SIDE_WIDTH: f32 = 352.;
/// Between the page's cards.
const GAP: f32 = 20.;
const RELEASES_URL: &str = "https://github.com/matty/VRFaceTracking/releases";

/// The setup steps whose place is taken by their own controls: allowing
/// debugging, installing the app, starting the stream and waiting for it to
/// arrive.
const ALLOW_STEP: usize = 2;
const INSTALL_STEP: usize = 3;
const START_STEP: usize = 4;
const RECEIVE_STEP: usize = 5;

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

impl Busy {
    /// What's happening, for a line while it takes.
    fn doing(self) -> SharedString {
        match self {
            Busy::Downloading => t!("headset.busy_downloading"),
            Busy::Connecting => t!("headset.busy_connecting"),
            Busy::Disconnecting => t!("headset.busy_disconnecting"),
            Busy::Installing => t!("headset.busy_installing"),
            Busy::Opening => t!("headset.busy_opening"),
            Busy::Starting => t!("headset.busy_starting"),
            Busy::Stopping => t!("headset.busy_stopping"),
            Busy::Restarting => t!("headset.busy_restarting"),
            Busy::ReadingLog => t!("headset.busy_reading_log"),
        }
        .into()
    }
}

/// How the page found the headset's Wi-Fi address without being told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FoundBy {
    /// The headset connected to adb reported it.
    Adb,
    /// VRFT's camera stream comes from it.
    Stream,
    /// It advertises adb over Wi-Fi on the network.
    Network,
}

/// The card an outcome is shown in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Area {
    Connection,
    App,
    Stream,
    /// Per-eye gaze, which restarts the stream but has its own card.
    EyeGaze,
    /// The five-camera stream, which does too.
    FiveCameras,
    Log,
}

/// What a finished action reports back.
#[derive(Default)]
struct Done {
    message: Option<String>,
    /// The stream was asked to start, so the page follows up on whether it
    /// reaches VRFT.
    started_stream: bool,
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
/// The adb to read with, the device chosen, and a Wi-Fi headset to ask again.
type PollTarget = (Arc<Adb>, Option<String>, Option<String>);

struct Snapshot {
    /// The device that was chosen when the reading started.
    requested: Option<String>,
    devices: Result<Vec<Device>, String>,
    selected: Option<String>,
    details: Option<Details>,
    /// Why the chosen headset's details couldn't be read.
    details_error: Option<String>,
    /// Where devices advertise adb over Wi-Fi, when that was looked for.
    advertised: Option<Vec<SocketAddr>>,
}

pub struct HeadsetPage {
    daemon: Entity<QuestProState>,
    app_dir: Option<PathBuf>,
    adb: AdbSetup,
    devices: Vec<Device>,
    /// Why adb couldn't list devices.
    devices_error: Option<String>,
    /// The device the page acts on.
    selected: Option<String>,
    details: Option<Details>,
    /// Why the chosen headset's details couldn't be read.
    details_error: Option<String>,
    /// Where devices on the network advertise adb over Wi-Fi.
    advertised: Vec<SocketAddr>,
    /// Advertised addresses not to connect to by themselves: tried once
    /// already, or disconnected by hand. Each is forgotten once it's no
    /// longer advertised, such as after the headset restarts.
    not_auto: Vec<SocketAddr>,
    /// When the chosen Wi-Fi headset was last asked to allow this computer.
    asked: Option<Instant>,
    /// The headset app this VRFT offers.
    package: Option<Package>,
    busy: Option<Busy>,
    message: Option<(Area, Notice)>,
    /// When a success notice stops being shown.
    message_expires: Option<Instant>,
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
    /// Per-eye gaze as last set here, shown while the headset app isn't
    /// streaming and so doesn't report it.
    eye_gaze_set: Option<bool>,
    /// The five-camera stream, likewise.
    five_cameras_set: Option<bool>,
    /// When Start stream was last pressed, until VRFT receives the stream.
    stream_started: Option<Instant>,
    /// The headset app's log shows at the bottom of the page.
    show_log: bool,
    /// The Wi-Fi card shows the devices and which adb is used.
    connection_details: bool,
    /// The headset app card shows the firmware build.
    app_details: bool,
    _poll: Task<()>,
    _action: Option<Task<()>>,
    _subscriptions: [Subscription; 2],
}

impl HeadsetPage {
    pub fn new(daemon: Entity<QuestProState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let address =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("headset.address_placeholder")));
        let subscriptions = [
            // The stream card shows what VRFT receives.
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
                if let Some((adb, requested, ask)) = next {
                    let snapshot = cx
                        .background_executor()
                        .spawn(async move { read_headset(&adb, requested, ask) })
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
            app_dir: vrft_gui_core::paths::app_dir(),
            adb: AdbSetup::Looking,
            devices: Vec::new(),
            devices_error: None,
            selected: None,
            details: None,
            details_error: None,
            advertised: Vec::new(),
            not_auto: Vec::new(),
            asked: None,
            package: None,
            busy: None,
            message: None,
            message_expires: None,
            installing: None,
            replace: None,
            log: None,
            address,
            suggested: None,
            watching: false,
            refresh_requested: false,
            eye_gaze_set: None,
            five_cameras_set: None,
            stream_started: None,
            show_log: false,
            connection_details: false,
            app_details: false,
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
                    // Platform-tools are downloaded into the data folder, which
                    // an installed copy's updates leave alone.
                    let data_dir = vrft_gui_core::paths::data_dir();
                    let app_dir = app_dir.as_deref();
                    (
                        Adb::locate(data_dir.as_deref()),
                        app_dir.and_then(headset_app::find),
                    )
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

    /// The adb to read the headset with, the device chosen, and a Wi-Fi
    /// headset to ask again to allow this computer, when a reading is due.
    fn poll_target(&mut self, last: Option<Instant>) -> Option<PollTarget> {
        let AdbSetup::Found(adb) = &self.adb else {
            return None;
        };
        let adb = adb.clone();
        let due = last.is_none_or(|at| at.elapsed() >= POLL_INTERVAL);
        if !self.watching || !(due || self.refresh_requested) {
            return None;
        }
        self.refresh_requested = false;
        let ask = self.waiting_on_wifi().filter(|_| {
            self.busy.is_none() && self.asked.is_some_and(|at| at.elapsed() >= ASK_AGAIN_AFTER)
        });
        if ask.is_some() {
            self.asked = Some(Instant::now());
        }
        Some((adb, self.selected.clone(), ask))
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
        // Once VRFT receives the stream, a start has nothing left to follow.
        if self
            .daemon
            .read(cx)
            .status()
            .is_some_and(|status| status.source.is_some())
        {
            // Along with what was said as it started.
            if self.stream_started.take().is_some()
                && matches!(self.message, Some((Area::Stream, _)))
            {
                self.message = None;
            }
        }
        // Someone may have chosen another device while this was read.
        if self.selected == snapshot.requested {
            // What was said about one headset doesn't hold for another.
            if snapshot.selected != self.selected {
                self.message = None;
                self.replace = None;
            }
            self.selected = snapshot.selected;
            self.details = snapshot.details;
            self.details_error = snapshot.details_error;
        }
        if let Some(advertised) = snapshot.advertised {
            self.not_auto.retain(|address| advertised.contains(address));
            self.advertised = advertised;
        }
        // The connection that found it waiting was the first request.
        if self.waiting_on_wifi().is_none() {
            self.asked = None;
        } else if self.asked.is_none() {
            self.asked = Some(Instant::now());
        }
        self.suggest_address(window, cx);
        self.connect_advertised(cx);
        cx.notify();
    }

    /// Connects to a headset that takes adb over Wi-Fi on this network, as
    /// adb's mDNS discovery finds it, so it's reachable without a cable or
    /// typing its address. Each address is tried once while it's advertised.
    fn connect_advertised(&mut self, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let listed = |address: &SocketAddr| {
            self.devices
                .iter()
                .any(|device| device.wireless_address() == Some(*address))
        };
        let Some(target) = self
            .advertised
            .iter()
            .copied()
            .find(|address| !self.not_auto.contains(address) && !listed(address))
        else {
            return;
        };
        self.not_auto.push(target);
        // Keep acting on a headset that's already usable, such as over USB.
        let select = self.ready_serial().is_none();
        self.run(
            Busy::Connecting,
            Area::Connection,
            move |adb| {
                let serial = match adb.connect(&target.to_string()) {
                    Ok(serial) => serial,
                    // The Allow USB debugging step says what to do.
                    Err(error) if error.is::<NotAllowed>() => return Ok(Done::default()),
                    Err(error) => return Err(error),
                };
                // Something other than a Quest Pro advertising adb here isn't
                // this page's to use.
                let other = adb.devices()?.into_iter().any(|device| {
                    device.serial == serial && device.is_ready() && !device.is_quest_pro()
                });
                if other {
                    adb.disconnect(&serial)?;
                    return Ok(Done::default());
                }
                Ok(Done {
                    select: select.then_some(serial),
                    ..Done::message(t!("headset.found_on_wifi", ip = target.ip()))
                })
            },
            cx,
        );
    }

    /// The headset's address as found without asking: from the headset itself
    /// over adb, or from where VRFT's camera stream comes from.
    fn detected_address(&self, cx: &App) -> Option<(IpAddr, FoundBy)> {
        let from_usb = self
            .details
            .as_ref()
            .and_then(|details| details.wifi_address)
            .map(|ip| (IpAddr::V4(ip), FoundBy::Adb));
        from_usb
            .or_else(|| {
                self.daemon
                    .read(cx)
                    .status()
                    .and_then(|status| status.source.as_deref())
                    .and_then(|source| source.parse::<SocketAddr>().ok())
                    .map(|address| address.ip())
                    .filter(|ip| !ip.is_loopback())
                    .map(|ip| (ip, FoundBy::Stream))
            })
            .or_else(|| {
                self.advertised
                    .first()
                    .map(|address| (address.ip(), FoundBy::Network))
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
    fn address_hint(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let typed = self.address.read(cx).value();
        let (ip, by) = self.detected_address(cx)?;
        let found = match by {
            FoundBy::Adb => t!("headset.found_from_headset"),
            FoundBy::Stream => t!("headset.found_from_stream"),
            FoundBy::Network => t!("headset.found_on_network"),
        };
        let found_at = match by {
            FoundBy::Adb => t!("headset.found_ip_from_headset", ip = ip),
            FoundBy::Stream => t!("headset.found_ip_from_stream", ip = ip),
            FoundBy::Network => t!("headset.found_ip_on_network", ip = ip),
        };
        if typed.trim() == ip.to_string() {
            return Some(
                h_flex()
                    .gap_1p5()
                    .text_xs()
                    .text_color(palette::text_3())
                    .child(
                        Icon::new(IconName::Check)
                            .size(px(12.))
                            .text_color(palette::text_2()),
                    )
                    .child(found)
                    .into_any_element(),
            );
        }
        Some(
            h_flex()
                .gap_2()
                .flex_wrap()
                .text_xs()
                .text_color(palette::text_3())
                .child(found_at)
                .child(
                    Button::new("use-detected-address")
                        .ghost()
                        .xsmall()
                        .label(t!("headset.use_ip", ip = ip))
                        .on_click(cx.listener(move |page, _, window, cx| {
                            page.use_detected(ip, window, cx)
                        })),
                )
                .into_any_element(),
        )
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

    /// Whether VRFT receives the headset's camera stream.
    fn received(&self, cx: &App) -> bool {
        self.daemon
            .read(cx)
            .status()
            .is_some_and(|status| status.source.is_some())
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
                // A started stream isn't a success until VRFT receives it.
                let tone = if done.started_stream {
                    self.stream_started = Some(Instant::now());
                    Tone::Waiting
                } else {
                    Tone::Good
                };
                // A success only needs reading once.
                self.message_expires =
                    (tone == Tone::Good).then(|| Instant::now() + SUCCESS_SHOWN_FOR);
                self.message = done
                    .message
                    .map(|message| (area, Notice::new(tone, message)));
            }
            Err(error) => {
                if matches!(
                    error.downcast_ref::<InstallError>(),
                    Some(InstallError::DifferentSigner | InstallError::Downgrade)
                ) {
                    self.replace = installing;
                }
                // The headset app logs what went wrong on its side.
                let notice = if matches!(
                    area,
                    Area::Stream | Area::App | Area::EyeGaze | Area::FiveCameras
                ) {
                    // The notice points at the log, so the log shows.
                    self.show_log = true;
                    let brief = error.to_string();
                    let full = format!("{error:#}");
                    let notice = Notice::new(
                        Tone::Problem,
                        t!("headset.error_see_log", error = brief.trim_end_matches('.')),
                    );
                    if full == brief {
                        notice
                    } else {
                        notice.details(full)
                    }
                } else {
                    Notice::error("", &error)
                };
                self.message = Some((area, notice));
                self.message_expires = None;
            }
        }
        self.refresh_requested = true;
        cx.notify();
    }

    fn fail(&mut self, area: Area, message: &str, cx: &mut Context<Self>) {
        self.message = Some((area, Notice::new(Tone::Problem, message.to_string())));
        cx.notify();
    }

    fn select(&mut self, serial: String, cx: &mut Context<Self>) {
        self.selected = Some(serial);
        self.details = None;
        self.refresh_requested = true;
        cx.notify();
    }

    fn download_adb(&mut self, cx: &mut Context<Self>) {
        let Some(data_dir) = vrft_gui_core::paths::data_dir() else {
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
                .spawn(async move { adb::download_platform_tools(&data_dir) })
                .await;
            this.update(cx, |page, cx| {
                page.busy = None;
                match result {
                    Ok(adb) => {
                        // Connection details say where it went.
                        page.message = Some((
                            Area::Connection,
                            Notice::new(Tone::Good, t!("headset.downloaded_tool")),
                        ));
                        page.adb = AdbSetup::Found(Arc::new(adb));
                        page.refresh_requested = true;
                    }
                    Err(error) => {
                        page.message = Some((Area::Connection, Notice::error("", &error)));
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
                    ..Done::message(t!("headset.connected_wifi_at", ip = ip))
                })
            },
            cx,
        );
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        let typed = self.address.read(cx).value();
        let Some(target) = adb::wireless_target(&typed) else {
            self.fail(Area::Connection, &t!("headset.enter_address"), cx);
            return;
        };
        self.run(
            Busy::Connecting,
            Area::Connection,
            move |adb| {
                let serial = adb.connect(&target)?;
                Ok(Done {
                    select: Some(serial),
                    ..Done::message(t!("headset.connected_to", target = target))
                })
            },
            cx,
        );
    }

    /// The chosen headset's serial, when it's reached over Wi-Fi but hasn't
    /// allowed this computer to debug it.
    fn waiting_on_wifi(&self) -> Option<String> {
        self.selected_device()
            .filter(|device| device.is_wireless() && device.state == DeviceState::Unauthorized)
            .map(|device| device.serial.clone())
    }

    fn disconnect(&mut self, serial: String, cx: &mut Context<Self>) {
        // A headset still advertising adb would otherwise be connected again.
        if let Ok(address) = serial.parse::<SocketAddr>() {
            if !self.not_auto.contains(&address) {
                self.not_auto.push(address);
            }
        }
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
                let package = package.describe();
                Ok(Done::message(if replace {
                    t!("headset.installed_replaced", package = package)
                } else if was_streaming {
                    t!("headset.installed_stream_stopped", package = package)
                } else {
                    t!("headset.installed", package = package)
                }))
            },
            cx,
        );
    }

    fn choose_package(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("headset.install_prompt").into()),
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
                    page.fail(Area::App, &t!("headset.choose_apk"), cx);
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
                Ok(Done {
                    started_stream: true,
                    ..Done::message(t!("headset.starting_stream"))
                })
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
                Ok(Done::message(t!("headset.stopped_message")))
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
        self.eye_gaze_set = Some(on);
        self.run(
            Busy::Restarting,
            Area::EyeGaze,
            move |adb| {
                let streaming = adb.streaming(&serial)?;
                if streaming {
                    adb.stop_stream(&serial)?;
                }
                adb.open_app(&serial, &[("eye_enabled", on), ("start_probe", streaming)])?;
                Ok(Done::message(match (on, streaming) {
                    (true, true) => t!("headset.eye_gaze_on_restarted"),
                    (false, true) => t!("headset.eye_gaze_off_restarted"),
                    (true, false) => t!("headset.eye_gaze_on_next"),
                    (false, false) => t!("headset.eye_gaze_off_next"),
                }))
            },
            cx,
        );
    }

    /// Saves the headset app's five-camera setting, which applies when its
    /// stream starts, so a running stream restarts.
    fn set_five_cameras(&mut self, on: bool, cx: &mut Context<Self>) {
        let Some(serial) = self.ready_serial() else {
            return;
        };
        self.five_cameras_set = Some(on);
        self.run(
            Busy::Restarting,
            Area::FiveCameras,
            move |adb| {
                let streaming = adb.streaming(&serial)?;
                if streaming {
                    adb.stop_stream(&serial)?;
                }
                adb.open_app(&serial, &[("five_cameras", on), ("start_probe", streaming)])?;
                Ok(Done::message(match (on, streaming) {
                    (true, true) => t!("headset.five_cameras_on_restarted"),
                    (false, true) => t!("headset.five_cameras_off_restarted"),
                    (true, false) => t!("headset.five_cameras_on_next"),
                    (false, false) => t!("headset.five_cameras_off_next"),
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

    /// Shows or hides the log, reading it the first time it shows.
    fn toggle_log(&mut self, cx: &mut Context<Self>) {
        self.show_log = !self.show_log;
        if self.show_log && self.log.is_none() {
            self.read_log(cx);
        }
        cx.notify();
    }

    /// The line under the page's title: how the headset is reached, and how
    /// far along setting it up is.
    fn description(&self, received: bool, remaining: usize) -> String {
        match &self.adb {
            AdbSetup::Looking => return t!("headset.looking_for_tool").into(),
            AdbSetup::Missing if !received => return t!("headset.download_tool_to_set_up").into(),
            _ => {}
        }
        if self.devices_error.is_some() && !received {
            return t!("headset.tool_not_working").into();
        }
        let Some(device) = self.selected_device() else {
            return if received {
                t!("headset.streaming_not_reachable").into()
            } else {
                t!("headset.not_reachable").into()
            };
        };
        let name = if device.is_quest_pro() {
            "Quest Pro".to_string()
        } else {
            device.name()
        };
        match &device.state {
            DeviceState::Ready => match (device.is_wireless(), remaining) {
                (true, 0) => t!("headset.wifi_streaming", name = name),
                (true, 1 | 2) => t!("headset.wifi_almost", name = name),
                (true, _) => t!("headset.wifi_linked", name = name),
                (false, 0) => t!("headset.usb_streaming", name = name),
                (false, 1 | 2) => t!("headset.usb_almost", name = name),
                (false, _) => t!("headset.usb_linked", name = name),
            }
            .into(),
            DeviceState::Unauthorized => t!("headset.waiting_for_debugging", name = name).into(),
            DeviceState::Offline => t!("headset.not_responding_replug", name = name).into(),
            DeviceState::Other(state) => {
                t!("headset.device_state", name = name, state = state).into()
            }
        }
    }

    /// How the USB or Wi-Fi link that controls the headset stands: its mark,
    /// its label and a word on it.
    fn control_link(&self) -> (Tone, String, String) {
        let control = |tone: Tone, caption: &str| {
            (
                tone,
                t!("headset.control").into_owned(),
                caption.to_string(),
            )
        };
        match &self.adb {
            AdbSetup::Looking => return control(Tone::Waiting, &t!("headset.looking")),
            AdbSetup::Missing => return control(Tone::Off, &t!("headset.tool_not_downloaded")),
            AdbSetup::Found(_) => {}
        }
        if self.devices_error.is_some() {
            return control(Tone::Problem, &t!("headset.not_working"));
        }
        let Some(device) = self.selected_device() else {
            return control(Tone::Off, &t!("headset.not_connected"));
        };
        let via = if device.is_wireless() {
            t!("headset.control_wifi")
        } else {
            t!("headset.control_usb")
        }
        .into_owned();
        match &device.state {
            DeviceState::Ready => (Tone::Good, via, t!("headset.installs_and_starts").into()),
            DeviceState::Unauthorized => {
                (Tone::Waiting, via, t!("headset.allow_usb_debugging").into())
            }
            DeviceState::Offline => control(Tone::Problem, &t!("headset.not_responding")),
            DeviceState::Other(state) => control(Tone::Waiting, state),
        }
    }

    /// How the camera stream from the headset to VRFT stands.
    fn camera_link(&self, received: bool, cx: &App) -> (Tone, String, String) {
        let label = t!("headset.camera_stream").into_owned();
        if received {
            let over_usb = self
                .daemon
                .read(cx)
                .status()
                .and_then(|status| status.source.as_deref())
                .and_then(|source| source.parse::<SocketAddr>().ok())
                .is_some_and(|address| address.ip().is_loopback());
            let caption = if over_usb {
                t!("headset.over_usb")
            } else {
                t!("headset.over_wifi")
            };
            return (Tone::Good, label, caption.into());
        }
        if self.busy == Some(Busy::Starting) {
            return (Tone::Waiting, label, t!("headset.starting").into());
        }
        let details = self
            .details
            .as_ref()
            .filter(|_| self.ready_serial().is_some());
        match details {
            Some(details) if details.streaming => {
                (Tone::Waiting, label, t!("headset.sent_not_received").into())
            }
            Some(_) => (Tone::Off, label, t!("headset.not_started").into()),
            None => (Tone::Off, label, t!("headset.not_receiving").into()),
        }
    }

    fn message_in(&self, area: Area) -> Option<Notice> {
        if self
            .message_expires
            .is_some_and(|expires| Instant::now() >= expires)
        {
            return None;
        }
        self.message
            .as_ref()
            .filter(|(shown_in, _)| *shown_in == area)
            .map(|(_, notice)| notice.clone())
    }

    /// This PC, the headset and VRFT, and how each link between them stands.
    fn chain(&self, received: bool, cx: &Context<Self>) -> impl IntoElement {
        let device = self.selected_device();
        let details = self
            .details
            .as_ref()
            .filter(|_| self.ready_serial().is_some());
        let control = self.control_link();
        let camera = self.camera_link(received, cx);
        let reached = control.0 == Tone::Good || received;
        let headset_name = device
            .filter(|device| !device.is_quest_pro())
            .map(|device| device.name())
            .unwrap_or_else(|| "Quest Pro".into());
        let headset_detail = match details.and_then(|details| details.battery) {
            Some(battery) => battery_text(battery, true),
            None if control.0 == Tone::Good => t!("headset.connected").into(),
            None if received => t!("headset.streaming").into(),
            None => t!("headset.not_connected").into(),
        };
        card(cx)
            .px(px(28.))
            .pt(px(20.))
            .pb_4()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(
                h_flex()
                    .items_center()
                    .child(chain_node(
                        IconName::Monitor,
                        t!("headset.this_pc").into(),
                        mono(t!(
                            "headset.vrft_version",
                            version = env!("CARGO_PKG_VERSION")
                        ))
                        .into_any_element(),
                        NodeLook::Plain,
                    ))
                    .child(chain_link(control))
                    .child(chain_node(
                        IconName::Glasses,
                        headset_name.into(),
                        div().child(headset_detail).into_any_element(),
                        if reached {
                            NodeLook::Lit
                        } else {
                            NodeLook::Dim
                        },
                    ))
                    .child(chain_link(camera))
                    .child(chain_node(
                        IconName::ScanFace,
                        "VRFaceTracking".into(),
                        div()
                            .child(if received {
                                t!("headset.receiving")
                            } else {
                                t!("headset.not_receiving")
                            })
                            .into_any_element(),
                        if received {
                            NodeLook::Lit
                        } else {
                            NodeLook::Dim
                        },
                    )),
            )
            .child(
                div()
                    .pt_3()
                    .border_t_1()
                    .border_color(palette::line_soft())
                    .text_xs()
                    .line_height(px(17.))
                    .text_color(palette::text_3())
                    .child(t!("headset.link_only_for_control")),
            )
    }

    /// The setup steps' inputs, as the page knows them now.
    fn setup_state(&self, received: bool) -> SetupState {
        let device = self.selected_device().map(|device| match device.state {
            DeviceState::Ready => SetupDevice::Ready,
            DeviceState::Unauthorized => SetupDevice::Unauthorized,
            _ => SetupDevice::NotResponding,
        });
        let details = self.details.as_ref();
        SetupState {
            adb: match self.adb {
                AdbSetup::Looking => None,
                AdbSetup::Missing => Some(false),
                AdbSetup::Found(_) => Some(true),
            },
            device,
            app_installed: details.map(|details| details.app.is_some()),
            streaming: details.map(|details| details.streaming),
            received,
        }
    }

    /// What's done of setting up the headset, the step to do now with its
    /// controls, and what's left, until VRFT receives the cameras.
    fn setup_card(
        &self,
        steps: &[SetupStep],
        current: usize,
        received: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let done = steps.iter().filter(|step| step.done).count();
        let total = steps.len();
        let progress = steps.iter().enumerate().map(|(index, step)| {
            div().w(px(16.)).h(px(3.)).rounded(px(2.)).bg(if step.done {
                palette::text()
            } else if index == current {
                palette::line_focus()
            } else {
                palette::line_strong()
            })
        });
        let rows: Vec<AnyElement> = steps
            .iter()
            .enumerate()
            .map(|(index, step)| {
                if step.done {
                    self.done_row(index, step)
                } else if index == current {
                    self.current_step(index, step, received, cx)
                } else {
                    pending_row(index, step, index > current + 1)
                }
            })
            .collect();
        // What starting the stream said, when the step it's about isn't the
        // one showing.
        let stream_notices = if matches!(current, START_STEP | RECEIVE_STEP) {
            Vec::new()
        } else {
            self.stream_notices(received)
        };
        card(cx)
            .px(px(20.))
            .pt(px(18.))
            .pb_2()
            .flex()
            .flex_col()
            .child(
                h_flex()
                    .gap_3()
                    .pb(px(14.))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(15.))
                            .font_semibold()
                            .text_color(palette::text())
                            .child(t!("headset.set_up")),
                    )
                    .child(h_flex().gap(px(3.)).children(progress))
                    .child(
                        mono(t!("headset.steps_done", done = done, total = total))
                            .text_size(px(11.5))
                            .text_color(palette::text_3()),
                    ),
            )
            .children(rows)
            .when(!stream_notices.is_empty(), |card| {
                card.child(v_flex().gap_2().pt_2().pb_2().children(stream_notices))
            })
            .into_any_element()
    }

    /// A step that's done, in one line with a word on how.
    fn done_row(&self, index: usize, step: &SetupStep) -> AnyElement {
        let details = self.details.as_ref();
        let detail: Option<(String, bool)> = match index {
            0 => match &self.adb {
                AdbSetup::Found(adb) if adb.origin() == Origin::App => {
                    Some(("Platform-Tools".into(), false))
                }
                AdbSetup::Found(_) => Some((t!("headset.found").into(), false)),
                _ => None,
            },
            1 => self.selected_device().map(|device| {
                let via = if device.is_wireless() { "Wi-Fi" } else { "USB" };
                (via.to_string(), false)
            }),
            2 => self
                .selected_device()
                .filter(|device| device.is_ready())
                .map(|_| (t!("headset.allowed").into(), false)),
            INSTALL_STEP => details
                .and_then(|details| details.app.as_ref())
                .map(|app| (app.version_name.clone(), true)),
            START_STEP => Some((t!("headset.streaming").into(), false)),
            _ => None,
        };
        h_flex()
            .h(px(40.))
            .gap_3()
            .border_t_1()
            .border_color(palette::line_soft())
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(18.))
                    .rounded_full()
                    .bg(step_grey())
                    .text_color(palette::text())
                    .child(Icon::new(IconName::Check).size(px(11.))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .text_color(palette::text_2())
                    .child(step.label.clone()),
            )
            .children(detail.map(|(text, numeric)| {
                let detail = if numeric {
                    mono(text).text_size(px(11.5))
                } else {
                    div().text_xs().child(text)
                };
                detail.flex_none().text_color(palette::text_3())
            }))
            .into_any_element()
    }

    /// The step to do now, raised, with what does it inside.
    fn current_step(
        &self,
        index: usize,
        step: &SetupStep,
        received: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let actions = self.step_actions(index, cx);
        let mut notices: Vec<AnyElement> = Vec::new();
        match index {
            0 if matches!(self.adb, AdbSetup::Missing) => notices.extend(
                self.message_in(Area::Connection)
                    .map(IntoElement::into_any_element),
            ),
            // Connecting is what this step waits on, so its outcome shows here.
            ALLOW_STEP => {
                if self.waiting_on_wifi().is_some() {
                    notices.push(
                        Notice::new(Tone::Waiting, t!("headset.wifi_not_allowed"))
                            .into_any_element(),
                    );
                }
                notices.extend(
                    self.message_in(Area::Connection)
                        .map(IntoElement::into_any_element),
                );
            }
            INSTALL_STEP => notices.extend(self.app_outcome(cx)),
            START_STEP | RECEIVE_STEP => notices.extend(
                self.stream_notices(received)
                    .into_iter()
                    .map(IntoElement::into_any_element),
            ),
            _ => {}
        }
        h_flex()
            .items_start()
            .gap_3()
            .my(px(6.))
            .mx(px(-10.))
            .px(px(10.))
            .py_4()
            .rounded(px(11.))
            .bg(palette::raised())
            .border_1()
            .border_color(palette::line_strong())
            .child(step_number(index + 1, true))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_3()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .font_semibold()
                                    .text_color(palette::text())
                                    .child(step.label.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .line_height(px(19.))
                                    .text_color(palette::text_2())
                                    .child(step.hint.clone()),
                            ),
                    )
                    .when(!actions.is_empty(), |body| {
                        body.child(h_flex().gap_2().flex_wrap().children(actions))
                    })
                    .children(notices),
            )
            .into_any_element()
    }

    /// The buttons that do the step to do now.
    fn step_actions(&self, index: usize, cx: &Context<Self>) -> Vec<AnyElement> {
        let busy = self.busy.is_some();
        let ready = self.ready_serial().is_some();
        let streaming = self
            .details
            .as_ref()
            .filter(|_| ready)
            .is_some_and(|details| details.streaming);
        match index {
            0 if matches!(self.adb, AdbSetup::Missing) => vec![
                Button::new("download-adb")
                    .primary()
                    .prominent()
                    .icon(IconName::Download)
                    .label(t!("headset.download"))
                    .tooltip(t!(
                        "headset.download_tooltip",
                        destination = self.download_destination()
                    ))
                    .loading(self.busy == Some(Busy::Downloading))
                    .disabled(busy || self.app_dir.is_none())
                    .on_click(cx.listener(|page, _, _, cx| page.download_adb(cx)))
                    .into_any_element(),
                terms_button("adb-terms").prominent().into_any_element(),
            ],
            INSTALL_STEP => {
                let install = self
                    .install_button("setup-install", cx)
                    .map(|(button, needed)| {
                        if needed { button.primary() } else { button }
                            .prominent()
                            .into_any_element()
                    });
                install
                    .into_iter()
                    .chain([Button::new("setup-install-file")
                        .ghost()
                        .prominent()
                        .icon(IconName::FolderOpen)
                        .label(t!("headset.from_file"))
                        .disabled(!ready || busy)
                        .on_click(cx.listener(|page, _, _, cx| page.choose_package(cx)))
                        .into_any_element()])
                    .collect()
            }
            START_STEP | RECEIVE_STEP => vec![
                if streaming {
                    self.stop_button("setup-stop-stream", cx)
                        .prominent()
                        .into_any_element()
                } else {
                    self.start_button("setup-start-stream", cx)
                        .prominent()
                        .into_any_element()
                },
                self.open_button("setup-open-app", cx)
                    .prominent()
                    .into_any_element(),
            ],
            _ => Vec::new(),
        }
    }

    fn start_button(&self, id: &'static str, cx: &Context<Self>) -> Button {
        let ready = self.ready_serial().is_some();
        let installed = self
            .details
            .as_ref()
            .filter(|_| ready)
            .is_some_and(|details| details.app.is_some());
        Button::new(id)
            .primary()
            .icon(IconName::Play)
            .label(t!("headset.start_stream"))
            .loading(self.busy == Some(Busy::Starting))
            .disabled(!ready || !installed || self.busy.is_some())
            .on_click(cx.listener(|page, _, _, cx| page.start_stream(cx)))
    }

    fn stop_button(&self, id: &'static str, cx: &Context<Self>) -> Button {
        let ready = self.ready_serial().is_some();
        Button::new(id)
            .danger()
            .outline()
            .icon(IconName::CircleStop)
            .label(t!("headset.stop_stream"))
            .loading(self.busy == Some(Busy::Stopping))
            .disabled(!ready || self.busy.is_some())
            .on_click(cx.listener(|page, _, _, cx| page.stop_stream(cx)))
    }

    fn open_button(&self, id: &'static str, cx: &Context<Self>) -> Button {
        let ready = self.ready_serial().is_some();
        let installed = self
            .details
            .as_ref()
            .filter(|_| ready)
            .is_some_and(|details| details.app.is_some());
        Button::new(id)
            .ghost()
            .label(t!("headset.open_on_headset"))
            .loading(self.busy == Some(Busy::Opening))
            .disabled(!ready || !installed || self.busy.is_some())
            .on_click(cx.listener(|page, _, _, cx| page.open_app(cx)))
    }

    /// After Start stream, what's keeping VRFT from receiving it, once it's
    /// had time to arrive; otherwise what the last stream action said.
    fn stream_notices(&self, received: bool) -> Vec<Notice> {
        let details = self
            .details
            .as_ref()
            .filter(|_| self.ready_serial().is_some());
        // A follow-up supersedes what was said as the stream started.
        match self.stream_follow_up(details, received) {
            Some(follow_up) => vec![follow_up],
            None => self.message_in(Area::Stream).into_iter().collect(),
        }
    }

    /// Once the headset streams to VRFT, the checklist gives way to this.
    fn stream_card(&self, received: bool, cx: &Context<Self>) -> AnyElement {
        let ready = self.ready_serial().is_some();
        let details = self.details.as_ref().filter(|_| ready);
        let streaming = details.is_some_and(|details| details.streaming);
        Panel::new(t!("headset.stream"))
            .child(stream_flow(
                match details {
                    Some(_) if streaming => (Tone::Good, t!("headset.streaming").into()),
                    Some(_) => (Tone::Off, t!("headset.stopped").into()),
                    None => (Tone::Off, "{2014}".into()),
                },
                if received {
                    (Tone::Good, t!("headset.receiving").into())
                } else {
                    (Tone::Off, t!("headset.not_receiving").into())
                },
            ))
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(if streaming {
                        self.stop_button("stop-stream", cx).regular()
                    } else {
                        self.start_button("start-stream", cx).regular()
                    })
                    .child(self.open_button("open-app", cx).regular()),
            )
            .children(self.stream_notices(received))
            .into_any_element()
    }

    /// After Start stream, what's keeping VRFT from receiving it, once it's
    /// had time to arrive.
    fn stream_follow_up(&self, details: Option<&Details>, received: bool) -> Option<Notice> {
        let started = self.stream_started?;
        if received || started.elapsed() < STREAM_GRACE {
            return None;
        }
        let streaming = details.is_some_and(|details| details.streaming);
        Some(if streaming {
            Notice::new(Tone::Problem, t!("headset.streaming_not_found"))
                .details(t!("headset.streaming_not_found_details"))
        } else {
            Notice::new(Tone::Problem, t!("headset.stream_didnt_start"))
        })
    }

    /// The button that installs the package this VRFT offers, labelled for
    /// how it compares with the headset's copy, and whether installing it is
    /// needed. The caller picks its look.
    fn install_button(&self, id: &'static str, cx: &Context<Self>) -> Option<(Button, bool)> {
        let ready = self.ready_serial().is_some();
        let details = self.details.as_ref().filter(|_| ready);
        let installed = details.and_then(|details| details.app.as_ref());
        let quest_pro = self
            .selected_device()
            .is_none_or(|device| device.is_quest_pro());
        let package = self.package.clone()?;
        let comparison = headset_app::compare(installed, &package);
        let version = package.describe();
        let (label, needed) = match (details.is_some(), comparison) {
            (true, Comparison::NotInstalled) => {
                (t!("headset.install_version", version = version), true)
            }
            (true, Comparison::Update) => {
                (t!("headset.update_to_version", version = version), true)
            }
            (true, Comparison::Same) => (t!("headset.reinstall"), false),
            (true, Comparison::Older) => (
                t!("headset.install_older_version", version = version),
                false,
            ),
            _ => (t!("headset.install_version", version = version), false),
        };
        let button = Button::new(id)
            .label(label)
            .loading(self.busy == Some(Busy::Installing))
            .disabled(!ready || self.busy.is_some())
            .on_click(cx.listener(move |page, _, _, cx| page.install(package.clone(), false, cx)));
        Some((button, needed && quest_pro))
    }

    /// What installing said, and the offer to uninstall first when Android
    /// wouldn't install over the headset's copy.
    fn app_outcome(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let ready = self.ready_serial().is_some();
        let busy = self.busy.is_some();
        let replace = self.replace.clone().map(|package| {
            v_flex()
                .gap_2()
                .items_start()
                .child(Notice::new(Tone::Waiting, t!("headset.reinstall_warning")))
                .child(
                    Button::new("replace")
                        .danger()
                        .outline()
                        .small()
                        .label(t!(
                            "headset.uninstall_and_install",
                            package = package.describe()
                        ))
                        .loading(self.busy == Some(Busy::Installing))
                        .disabled(!ready || busy)
                        .on_click(cx.listener(move |page, _, _, cx| {
                            page.install(package.clone(), true, cx)
                        })),
                )
                .into_any_element()
        });
        replace
            .into_iter()
            .chain(
                self.message_in(Area::App)
                    .map(IntoElement::into_any_element),
            )
            .collect()
    }

    fn app_card(&self, current: Option<usize>, cx: &Context<Self>) -> AnyElement {
        let ready = self.ready_serial().is_some();
        let busy = self.busy.is_some();
        let details = self.details.as_ref().filter(|_| ready);
        let installed = details.and_then(|details| details.app.as_ref());
        let on_headset: AnyElement = match (details, installed) {
            (_, Some(app)) => mono(app.version_name.clone()).into_any_element(),
            (Some(_), None) => div().child(t!("headset.not_installed")).into_any_element(),
            (None, _) => div().child("\u{2014}").into_any_element(),
        };
        let (offered_label, offered): (SharedString, AnyElement) = match &self.package {
            Some(package) => (
                match package.origin {
                    PackageOrigin::Bundled => t!("headset.comes_with_vrft"),
                    PackageOrigin::LocalBuild => t!("headset.local_build"),
                    PackageOrigin::Chosen => t!("headset.chosen_file"),
                }
                .into(),
                mono(package.describe()).into_any_element(),
            ),
            None => (
                t!("headset.available").into(),
                div().child(t!("headset.none")).into_any_element(),
            ),
        };
        let quest_pro = self
            .selected_device()
            .is_none_or(|device| device.is_quest_pro());
        let older = self.package.as_ref().and_then(|package| {
            (headset_app::compare(installed, package) == Comparison::Older)
                .then(|| Notice::new(Tone::Waiting, t!("headset.headset_app_newer")))
        });
        // The step to do now has its own white button.
        let step_has_primary = matches!(current, Some(INSTALL_STEP | START_STEP));
        let install = self.install_button("install", cx).map(|(button, needed)| {
            if needed && !step_has_primary {
                button.primary()
            } else {
                button
            }
            .small()
        });
        // The daemon found the headset app but can't read its stream.
        let mismatch = self
            .daemon
            .read(cx)
            .status()
            .and_then(|status| status.headset_mismatch.clone());
        let mismatch_notice = mismatch.map(|mismatch| {
            let version = mismatch.apk_version.as_deref();
            let text = match (mismatch.update, self.package.is_some(), version) {
                (Update::Vrft, _, Some(version)) => {
                    t!("headset.app_version_too_new", version = version)
                }
                (Update::Vrft, _, None) => t!("headset.app_too_new"),
                (Update::HeadsetApp, true, Some(version)) => {
                    t!("headset.app_version_too_old_install", version = version)
                }
                (Update::HeadsetApp, true, None) => t!("headset.app_too_old_install"),
                (Update::HeadsetApp, false, Some(version)) => {
                    t!("headset.app_version_too_old_releases", version = version)
                }
                (Update::HeadsetApp, false, None) => t!("headset.app_too_old_releases"),
            };
            Notice::new(Tone::Problem, text)
        });
        let mut notices: Vec<AnyElement> = Vec::new();
        notices.extend(mismatch_notice.map(IntoElement::into_any_element));
        if ready && !quest_pro {
            notices
                .push(Notice::new(Tone::Problem, t!("headset.not_a_quest_pro")).into_any_element());
        }
        notices.extend(older.map(IntoElement::into_any_element));
        if let Some(error) = self.details_error.clone().filter(|_| ready) {
            notices.push(
                Notice::new(Tone::Problem, t!("headset.couldnt_read_headset"))
                    .details(error)
                    .into_any_element(),
            );
        }
        let battery = details
            .and_then(|details| details.battery)
            .map(|battery| battery_text(battery, false))
            .unwrap_or_else(|| "{2014}".into());
        let firmware = details
            .and_then(|details| details.build.clone())
            .unwrap_or_else(|| "{2014}".into());
        // Installing from the setup step says how it went there.
        let outcome = if current == Some(INSTALL_STEP) {
            Vec::new()
        } else {
            self.app_outcome(cx)
        };
        card(cx)
            .px(px(18.))
            .pt_4()
            .pb(px(14.))
            .flex()
            .flex_col()
            .child(card_title(t!("headset.headset_app")).pb(px(10.)))
            .when(!notices.is_empty(), |card| {
                card.child(v_flex().gap_2().pb_3().children(notices))
            })
            .child(info(t!("headset.on_the_headset"), on_headset))
            .child(info(offered_label, offered))
            .child(info(t!("headset.battery"), div().child(battery)))
            .child(
                disclosure("app-details", t!("headset.details"), self.app_details)
                    .h(px(34.))
                    .text_size(px(12.5))
                    .hover(|style| style.text_color(palette::text()))
                    .on_click(cx.listener(|page, _, _, cx| {
                        page.app_details = !page.app_details;
                        cx.notify();
                    })),
            )
            .when(self.app_details, |card| {
                card.child(info(t!("headset.firmware"), mono(firmware)))
            })
            .child(
                h_flex()
                    .gap(px(6.))
                    .flex_wrap()
                    .pt_3()
                    .border_t_1()
                    .border_color(palette::line_soft())
                    .children(install)
                    .child(
                        Button::new("install-file")
                            .ghost()
                            .small()
                            .label(t!("headset.from_file"))
                            .disabled(!ready || busy)
                            .on_click(cx.listener(|page, _, _, cx| page.choose_package(cx))),
                    )
                    .child(
                        div().ml_auto().child(
                            Button::new("releases")
                                .ghost()
                                .small()
                                .label(t!("headset.releases"))
                                .icon(IconName::ExternalLink)
                                .on_click(|_, _, cx| cx.open_url(RELEASES_URL)),
                        ),
                    ),
            )
            .when(!outcome.is_empty(), |card| {
                card.child(v_flex().gap_2().pt_3().children(outcome))
            })
            .into_any_element()
    }

    /// Per-eye gaze: its switch, what turning it on does, and whether this
    /// firmware supports it. Only once the headset app is installed.
    fn eye_gaze_card(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let ready = self.ready_serial().is_some();
        let busy = self.busy.is_some();
        let details = self.details.as_ref().filter(|_| ready);
        if !details.is_some_and(|details| details.app.is_some()) {
            return None;
        }
        // The headset app reports its eye gaze setting only while VRFT
        // receives its stream.
        let enabled = self
            .daemon
            .read(cx)
            .status()
            .filter(|status| status.source.is_some())
            .and_then(|status| status.headset.as_ref())
            .and_then(|headset| headset.eye.as_ref())
            .map(|eye| eye.enabled);
        let (support_tone, support) =
            eye_firmware_support(details.and_then(|details| details.build.as_deref()));
        // Without the stream, what was last set here, else the headset
        // app's default, which is on.
        let checked = enabled.or(self.eye_gaze_set).unwrap_or(true);
        let switch = Switch::new("headset-eye-gaze")
            .accessibility_label(t!("headset.per_eye_gaze"))
            .checked(checked)
            .disabled(!ready || busy)
            .on_change(cx.listener(|page, checked: &bool, _, cx| page.set_eye_gaze(*checked, cx)));
        Some(
            card(cx)
                .px(px(18.))
                .py_4()
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(
                    h_flex()
                        .gap_2()
                        .child(card_title(t!("headset.per_eye_gaze")))
                        .child(
                            mono(t!("headset.experimental"))
                                .text_size(px(10.))
                                .px(px(6.))
                                .py(px(2.))
                                .rounded(px(4.))
                                .border_1()
                                .border_color(step_grey())
                                .text_color(palette::text_2()),
                        )
                        .child(div().flex_1())
                        .child(switch),
                )
                .child(
                    v_flex()
                        .gap_1()
                        .text_size(px(12.5))
                        .line_height(px(18.))
                        .text_color(palette::text_3())
                        .child(t!("headset.eye_gaze_description"))
                        .child(t!("headset.eye_gaze_note")),
                )
                .child(StatusLine::new(support_tone, support))
                .children(self.message_in(Area::EyeGaze))
                .into_any_element(),
        )
    }

    /// The five-camera stream: its switch and what it's for. Only once the
    /// headset app is installed.
    fn five_cameras_card(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let ready = self.ready_serial().is_some();
        let busy = self.busy.is_some();
        if !self
            .details
            .as_ref()
            .filter(|_| ready)
            .is_some_and(|details| details.app.is_some())
        {
            return None;
        }
        // Reported only while VRFT receives the stream; off by default.
        let enabled = self
            .daemon
            .read(cx)
            .status()
            .filter(|status| status.source.is_some())
            .and_then(|status| status.headset.as_ref())
            .and_then(|headset| headset.five_cameras);
        let checked = enabled.or(self.five_cameras_set).unwrap_or(false);
        let switch = Switch::new("headset-five-cameras")
            .accessibility_label(t!("headset.all_five_cameras"))
            .checked(checked)
            .disabled(!ready || busy)
            .on_change(
                cx.listener(|page, checked: &bool, _, cx| page.set_five_cameras(*checked, cx)),
            );
        Some(
            card(cx)
                .px(px(18.))
                .py_4()
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(
                    h_flex()
                        .gap_2()
                        .child(card_title(t!("headset.all_five_cameras")))
                        .child(div().flex_1())
                        .child(switch),
                )
                .child(
                    v_flex()
                        .gap_1()
                        .text_size(px(12.5))
                        .line_height(px(18.))
                        .text_color(palette::text_3())
                        .child(t!("headset.five_cameras_description"))
                        .child(t!("headset.five_cameras_note")),
                )
                .children(self.message_in(Area::FiveCameras))
                .into_any_element(),
        )
    }

    /// Whether the chosen USB headset can also be reached over Wi-Fi, at an
    /// address adb isn't already connected to.
    fn offer_wifi(&self) -> bool {
        let usb = self
            .selected_device()
            .is_some_and(|device| device.is_ready() && !device.is_wireless());
        if !usb {
            return false;
        }
        let Some(ip) = self
            .details
            .as_ref()
            .and_then(|details| details.wifi_address)
        else {
            return false;
        };
        // A Wi-Fi connection the headset hasn't allowed doesn't count: once
        // it's allowed over USB, Use Wi-Fi makes that connection again.
        !self
            .devices
            .iter()
            .filter(|device| device.is_ready())
            .filter_map(|device| device.wireless_address())
            .any(|address| address.ip() == IpAddr::V4(ip))
    }

    /// Wi-Fi, and behind Connection details the
    /// devices adb sees and which adb it is.
    fn connection_card(&self, current: Option<usize>, cx: &Context<Self>) -> AnyElement {
        let busy = self.busy.is_some();
        let connecting = self.busy == Some(Busy::Connecting);
        let found = matches!(self.adb, AdbSetup::Found(_));
        let on_wifi = self
            .selected_device()
            .is_some_and(|device| device.is_wireless() && device.is_ready());
        // Downloading, and asking again to allow debugging, from the setup
        // steps say how it went there.
        let messages_here = !(matches!(self.adb, AdbSetup::Missing) && current == Some(0)
            || current == Some(ALLOW_STEP));
        let body =
            if found {
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Input::new(&self.address).small()),
                            )
                            .child(
                                Button::new("connect-wifi")
                                    .small()
                                    .label(t!("headset.connect"))
                                    .loading(connecting)
                                    .disabled(busy)
                                    .on_click(cx.listener(|page, _, _, cx| page.connect(cx))),
                            ),
                    )
                    .children(self.address_hint(cx))
            } else {
                v_flex().child(div().text_xs().text_color(palette::text_3()).child(
                    match self.adb {
                        AdbSetup::Looking => t!("headset.looking_for_tool"),
                        _ => t!("headset.download_tool_first"),
                    },
                ))
            };
        card(cx)
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                h_flex()
                    .gap_2p5()
                    .pl(px(18.))
                    .pr(px(14.))
                    .py_3()
                    .child(
                        div()
                            .flex_none()
                            .text_color(palette::text_2())
                            .child(Icon::new(IconName::Wifi).size(px(15.))),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_medium()
                                    .text_color(palette::text())
                                    .child("Wi-Fi"),
                            )
                            .when(on_wifi, |title| {
                                title.child(
                                    div()
                                        .text_xs()
                                        .text_color(palette::text_3())
                                        .child(t!("headset.connected_over_wifi")),
                                )
                            }),
                    )
                    .when(self.offer_wifi(), |row| {
                        row.child(
                            Button::new("use-wifi")
                                .small()
                                .label(t!("headset.use_wifi"))
                                .tooltip(t!("headset.use_wifi_tooltip"))
                                .loading(connecting)
                                .disabled(busy)
                                .on_click(cx.listener(|page, _, _, cx| page.use_wifi(cx))),
                        )
                    }),
            )
            .child(
                body.pl(px(18.))
                    .pr(px(14.))
                    .pb_3()
                    // The whole error is under Connection details.
                    .when_some(
                        self.devices_error.clone().filter(|_| found),
                        |body, error| {
                            body.child(
                                Notice::new(Tone::Problem, t!("headset.tool_not_working"))
                                    .details(error),
                            )
                        },
                    )
                    .when(messages_here, |body| {
                        body.children(self.message_in(Area::Connection))
                    }),
            )
            .child(
                disclosure(
                    "connection-details",
                    t!("headset.connection_details"),
                    self.connection_details,
                )
                .pl(px(18.))
                .pr(px(14.))
                .py_3()
                .hover(|style| style.bg(palette::inset()))
                .on_click(cx.listener(|page, _, _, cx| {
                    page.connection_details = !page.connection_details;
                    cx.notify();
                })),
            )
            .when(self.connection_details, |card| {
                card.child(self.connection_details_body(current, cx))
            })
            .into_any_element()
    }

    /// The devices adb sees, which adb is used and from where, and getting
    /// VRFT its own copy.
    fn connection_details_body(&self, current: Option<usize>, cx: &Context<Self>) -> AnyElement {
        let busy = self.busy.is_some();
        let body = v_flex()
            .gap_3()
            .pl(px(18.))
            .pr(px(14.))
            .pb(px(14.))
            .text_xs()
            .text_color(palette::text_3());
        let adb = match &self.adb {
            AdbSetup::Looking => {
                return body.child(t!("headset.looking_for_adb")).into_any_element()
            }
            AdbSetup::Missing => {
                // The setup step offers the download while it's the step
                // to do; otherwise it's here.
                let offer = current != Some(0) && self.app_dir.is_some();
                return body
                    .child(t!(
                        "headset.adb_not_downloaded",
                        destination = self.download_destination()
                    ))
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .when(offer, |row| {
                                row.child(
                                    Button::new("download-adb-here")
                                        .small()
                                        .icon(IconName::Download)
                                        .label(t!("headset.download"))
                                        .loading(self.busy == Some(Busy::Downloading))
                                        .disabled(busy)
                                        .on_click(
                                            cx.listener(|page, _, _, cx| page.download_adb(cx)),
                                        ),
                                )
                            })
                            .child(terms_button("adb-terms-details").xsmall()),
                    )
                    .into_any_element();
            }
            AdbSetup::Found(adb) => adb.clone(),
        };
        let several = self.devices.len() > 1;
        let rows: Vec<AnyElement> = self
            .devices
            .iter()
            .enumerate()
            .map(|(index, device)| {
                let selected = self.selected.as_deref() == Some(device.serial.as_str());
                self.device_row(index, device, selected, several, cx)
            })
            .collect();
        body.when_some(self.devices_error.clone(), |body, error| {
            body.child(
                mono(error)
                    .text_size(px(11.5))
                    .text_color(palette::text_3()),
            )
        })
        .when(rows.is_empty() && self.devices_error.is_none(), |body| {
            body.child(t!("headset.no_headset_found"))
        })
        .when(!rows.is_empty(), |body| {
            body.child(v_flex().gap_1().children(rows))
        })
        .child(
            h_flex()
                .gap_2()
                .flex_wrap()
                .child(
                    div()
                        .id("adb-in-use")
                        .min_w_0()
                        .truncate()
                        .tooltip({
                            let path = adb.path().display().to_string();
                            move |window, cx| Tooltip::new(path.clone()).build(window, cx)
                        })
                        .child(adb.origin().label()),
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
                                .label(t!("headset.download_to_vrft_folder"))
                                .tooltip(t!(
                                    "headset.download_to_vrft_folder_tooltip",
                                    destination = self.download_destination()
                                ))
                                .loading(self.busy == Some(Busy::Downloading))
                                .disabled(busy)
                                .on_click(cx.listener(|page, _, _, cx| page.download_adb(cx))),
                        )
                        .child(terms_button("adb-terms-details").xsmall())
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
            .unwrap_or_else(|| t!("headset.the_vrft_folder").into())
    }

    fn device_row(
        &self,
        index: usize,
        device: &Device,
        selected: bool,
        several: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let busy = self.busy.is_some();
        let (tone, state) = match &device.state {
            DeviceState::Ready => (Tone::Good, t!("headset.connected").into_owned()),
            DeviceState::Unauthorized => {
                (Tone::Waiting, t!("headset.waiting_for_permission").into())
            }
            DeviceState::Offline => (Tone::Problem, t!("headset.replug_headset").into()),
            DeviceState::Other(state) => (Tone::Waiting, state.clone()),
        };
        let link = if device.is_wireless() {
            format!("Wi-Fi \u{b7} {}", device.serial)
        } else {
            format!("USB \u{b7} {}", device.serial)
        };
        let serial = device.serial.clone();
        let disconnect_serial = device.serial.clone();
        h_flex()
            .items_start()
            .gap_2p5()
            .mx(px(-8.))
            .px_2()
            .py_2()
            .rounded(px(8.))
            .when(selected && several, |row| {
                row.bg(palette::raised())
                    .border_1()
                    .border_color(palette::line_strong())
            })
            .child(
                div()
                    .flex_none()
                    .pt(px(2.))
                    .text_color(palette::text_3())
                    .child(
                        Icon::new(if device.is_wireless() {
                            IconName::Wifi
                        } else {
                            IconName::Usb
                        })
                        .size(px(14.)),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_medium()
                            .text_color(palette::text())
                            .child(device.name()),
                    )
                    .child(
                        mono(link)
                            .text_size(px(11.5))
                            .text_color(palette::text_3())
                            .truncate(),
                    )
                    .child(StatusLine::new(tone, state)),
            )
            .when(several && !selected, |row| {
                row.child(
                    Button::new(("use-device", index))
                        .ghost()
                        .xsmall()
                        .label(t!("headset.use"))
                        .disabled(busy)
                        .on_click(
                            cx.listener(move |page, _, _, cx| page.select(serial.clone(), cx)),
                        ),
                )
            })
            .when(device.is_wireless(), |row| {
                row.child(
                    Button::new(("disconnect", index))
                        .ghost()
                        .xsmall()
                        .label(t!("headset.disconnect"))
                        .loading(self.busy == Some(Busy::Disconnecting))
                        .disabled(busy)
                        .on_click(cx.listener(move |page, _, _, cx| {
                            page.disconnect(disconnect_serial.clone(), cx)
                        })),
                )
            })
            .into_any_element()
    }

    fn log_card(&self, cx: &Context<Self>) -> AnyElement {
        let ready = self.ready_serial().is_some();
        let busy = self.busy.is_some();
        let log = self.log.clone();
        Panel::new(t!("headset.headset_log"))
            .trailing(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("read-log")
                            .small()
                            .icon(if log.is_some() {
                                IconName::RefreshCw
                            } else {
                                IconName::ScrollText
                            })
                            .label(if log.is_some() {
                                t!("headset.refresh")
                            } else {
                                t!("headset.show_log")
                            })
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
                                .label(t!("headset.copy"))
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
                        .text_color(palette::text_3())
                        .child(t!("headset.log_empty"))
                        .into_any_element()
                } else {
                    div()
                        .id("headset-log")
                        .max_h(px(320.))
                        .overflow_y_scroll()
                        .p_3()
                        .rounded(px(10.))
                        .border_1()
                        .border_color(palette::line())
                        .bg(palette::sunken())
                        .font_family(MONO_FONT)
                        .text_size(px(11.5))
                        .text_color(palette::text_2())
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
        let two_columns = vrft_gui_core::content_width(window) >= TWO_COLUMN_WIDTH;
        let found = matches!(self.adb, AdbSetup::Found(_));
        let received = self.received(cx);
        let steps = setup_steps(&self.setup_state(received));
        let current = steps.iter().position(|step| !step.done);
        let remaining = steps.iter().filter(|step| !step.done).count();
        // Once every step is done the checklist gives way to the stream.
        let main = match current {
            Some(current) => self.setup_card(&steps, current, received, cx),
            None => self.stream_card(received, cx),
        };
        let mut side: Vec<AnyElement> = Vec::new();
        if found {
            side.push(self.app_card(current, cx));
            side.extend(self.five_cameras_card(cx));
            side.extend(self.eye_gaze_card(cx));
        }
        side.push(self.connection_card(current, cx));
        let columns = if two_columns {
            h_flex()
                .items_start()
                .gap(px(GAP))
                .child(v_flex().flex_1().min_w_0().child(main))
                .child(
                    v_flex()
                        .w(px(SIDE_WIDTH))
                        .flex_none()
                        .gap_4()
                        .children(side),
                )
        } else {
            v_flex().gap_4().child(main).children(side)
        };
        v_flex()
            .gap(px(GAP))
            .child(
                PageHeader::new(t!("headset.title"))
                    .description(self.description(received, remaining))
                    .trailing(
                        Button::new("toggle-log")
                            .ghost()
                            .regular()
                            .icon(IconName::FileText)
                            .label(t!("headset.headset_log"))
                            .tooltip(if self.show_log {
                                t!("headset.hide_log_tooltip")
                            } else {
                                t!("headset.show_log_tooltip")
                            })
                            .disabled(!found)
                            .on_click(cx.listener(|page, _, _, cx| page.toggle_log(cx))),
                    ),
            )
            .children(
                self.busy
                    .map(|busy| StatusLine::new(Tone::Waiting, busy.doing())),
            )
            .child(self.chain(received, cx))
            .child(columns)
            .when(found && self.show_log, |page| page.child(self.log_card(cx)))
    }
}

/// How a node of the connection chain is drawn: VRFT's own end, a part
/// that's reached, or one that isn't.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NodeLook {
    Plain,
    Lit,
    Dim,
}

/// One end of a link in the connection chain: an icon tile, a name, and a
/// line on how it stands.
fn chain_node(
    icon: IconName,
    name: SharedString,
    detail: AnyElement,
    look: NodeLook,
) -> impl IntoElement {
    let tile = div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(40.))
        .rounded(px(11.));
    let tile = match look {
        NodeLook::Lit => tile.bg(palette::text()).text_color(palette::page()),
        NodeLook::Plain | NodeLook::Dim => tile
            .bg(palette::raised())
            .border_1()
            .border_color(palette::line_strong())
            .text_color(if look == NodeLook::Dim {
                palette::text_3()
            } else {
                palette::text()
            }),
    };
    h_flex()
        .flex_none()
        .gap_3()
        .child(tile.child(Icon::new(icon).size(px(18.))))
        .child(
            v_flex()
                .gap(px(1.))
                .child(
                    div()
                        .text_size(px(13.5))
                        .font_semibold()
                        .text_color(palette::text())
                        .child(name),
                )
                .child(div().text_xs().text_color(palette::text_3()).child(detail)),
        )
}

/// The link between two nodes: its label over a line that's solid white
/// when it works and dim and dashed when it doesn't, and a word under it.
fn chain_link((tone, label, caption): (Tone, String, String)) -> impl IntoElement {
    let works = tone == Tone::Good;
    v_flex()
        .flex_1()
        .min_w(px(64.))
        .items_center()
        .gap(px(5.))
        .px(px(18.))
        .child(
            h_flex()
                .w_full()
                .justify_center()
                .gap_1p5()
                .text_xs()
                .text_color(if works {
                    rgb(0xd4d4d4).into()
                } else {
                    palette::text_2()
                })
                .child(if works {
                    Icon::new(IconName::Check).size(px(12.)).into_any_element()
                } else {
                    StatusDot::new(tone).into_any_element()
                })
                .child(div().min_w_0().truncate().child(label)),
        )
        .child(link_line(works))
        .child(
            div()
                .w_full()
                .text_center()
                .truncate()
                .text_size(px(11.5))
                .text_color(if tone == Tone::Problem {
                    palette::signal_text()
                } else {
                    palette::text_3()
                })
                .child(caption),
        )
}

fn link_line(works: bool) -> AnyElement {
    if works {
        return div()
            .w_full()
            .h(px(2.))
            .rounded(px(1.))
            .bg(palette::text())
            .into_any_element();
    }
    div()
        .w_full()
        .h(px(2.))
        .child(
            canvas(
                |_, _, _| {},
                |bounds: Bounds<Pixels>, _, window, _| {
                    let y = bounds.origin.y + px(1.);
                    let mut line = PathBuilder::stroke(px(2.)).dash_array(&[px(5.), px(4.)]);
                    line.move_to(point(bounds.origin.x, y));
                    line.line_to(point(bounds.origin.x + bounds.size.width, y));
                    if let Ok(path) = line.build() {
                        window.paint_path(path, dim_line());
                    }
                },
            )
            .size_full(),
        )
        .into_any_element()
}

/// A link that doesn't work yet: dimmer than a card's lines.
fn dim_line() -> Hsla {
    rgb(0x34343a).into()
}

/// Between the lines and the quiet text: a done step's disc, a pending
/// step's ring, a tag's edge.
fn step_grey() -> Hsla {
    rgb(0x3a3a41).into()
}

/// A step's number in a ring, white for the step to do now.
fn step_number(number: usize, current: bool) -> impl IntoElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(18.))
        .mt(px(1.))
        .rounded_full()
        .when(current, |mark| {
            mark.border_2().border_color(palette::text())
        })
        .when(!current, |mark| mark.border_1().border_color(step_grey()))
        .font_family(MONO_FONT)
        .text_size(px(10.))
        .when(current, |mark| mark.font_semibold())
        .text_color(if current {
            palette::text()
        } else {
            palette::text_3()
        })
        .child(number.to_string())
}

/// A step still to come, with what it will ask.
fn pending_row(index: usize, step: &SetupStep, ruled: bool) -> AnyElement {
    h_flex()
        .items_start()
        .gap_3()
        .pt_3()
        .pb(px(14.))
        .when(ruled, |row| {
            row.border_t_1().border_color(palette::line_soft())
        })
        .child(step_number(index + 1, false))
        .child(
            v_flex()
                .min_w_0()
                .gap_0p5()
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(palette::text_2())
                        .child(step.label.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::text_3())
                        .child(step.hint.clone()),
                ),
        )
        .into_any_element()
}

fn card_title(title: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(14.))
        .font_semibold()
        .text_color(palette::text())
        .child(title.into())
}

/// A label and its value on a line of a card, under a rule.
fn info(label: impl Into<SharedString>, value: impl IntoElement) -> Div {
    h_flex()
        .min_h(px(34.))
        .py(px(6.))
        .gap_3()
        .justify_between()
        .border_t_1()
        .border_color(palette::line_soft())
        .text_size(px(12.5))
        .child(
            div()
                .flex_none()
                .text_color(palette::text_3())
                .child(label.into()),
        )
        .child(
            div()
                .min_w_0()
                .text_right()
                .text_color(palette::text())
                .child(value),
        )
}

/// A row that shows or hides what's under it, with a chevron saying which.
/// The caller sets its padding.
fn disclosure(id: &'static str, label: impl Into<SharedString>, open: bool) -> Stateful<Div> {
    h_flex()
        .id(id)
        .gap_2p5()
        .border_t_1()
        .border_color(palette::line_soft())
        .cursor_pointer()
        .text_size(px(13.))
        .text_color(palette::text_2())
        .child(div().flex_1().min_w_0().child(label.into()))
        .child(
            div().flex_none().text_color(palette::text_3()).child(
                Icon::new(if open {
                    IconName::ChevronUp
                } else {
                    IconName::ChevronDown
                })
                .size(px(14.)),
            ),
        )
}

/// The battery's level, and whether it's charging after a middle dot when
/// `dotted`, or a comma otherwise.
fn battery_text(battery: adb::Battery, dotted: bool) -> String {
    match (battery.charging, dotted) {
        (true, true) => t!("headset.battery_charging_dotted", percent = battery.percent).into(),
        (true, false) => t!("headset.battery_charging", percent = battery.percent).into(),
        (false, _) => format!("{}%", battery.percent),
    }
}

/// The stream as it flows: the headset sending it, then VRFT receiving it,
/// each with a mark for how it stands.
fn stream_flow(headset: (Tone, SharedString), vrft: (Tone, SharedString)) -> impl IntoElement {
    let end = |name: SharedString, (tone, state): (Tone, SharedString)| {
        h_flex()
            .gap_2()
            .child(StatusDot::new(tone))
            .child(div().font_medium().text_color(palette::text()).child(name))
            .child(div().text_color(palette::text_3()).child(state))
    };
    h_flex()
        .gap_3()
        .flex_wrap()
        .text_size(px(13.))
        .child(end(t!("headset.headset").into(), headset))
        .child(
            div()
                .text_color(palette::text_4())
                .child(Icon::new(IconName::ChevronRight).size(px(14.))),
        )
        .child(end("VRFaceTracking".into(), vrft))
}

/// Opens the terms that downloading Platform-Tools comes under.
fn terms_button(id: &'static str) -> Button {
    Button::new(id)
        .ghost()
        .icon(IconName::ExternalLink)
        .label(t!("headset.android_sdk_terms"))
        .on_click(|_, _, cx| cx.open_url(adb::PLATFORM_TOOLS_TERMS_URL))
}

/// Lists adb's devices, keeps or picks the one to act on, and reads its
/// details.
fn read_headset(adb: &Adb, requested: Option<String>, ask: Option<String>) -> Snapshot {
    // A fresh request, which a headset takes once Allow has been chosen.
    // Until then it only asks again.
    if let Some(target) = ask {
        if let Err(error) = adb.connect(&target) {
            log::debug!("Asked {target} again to allow debugging: {error:#}");
        }
    }
    let devices = match adb.devices() {
        Ok(devices) => devices,
        Err(error) => {
            return Snapshot {
                requested,
                devices: Err(format!("{error:#}")),
                selected: None,
                details: None,
                details_error: None,
                advertised: None,
            }
        }
    };
    // Looked for only while no headset is reachable over Wi-Fi. An adb
    // without mDNS discovery finds none.
    let advertised = (!devices
        .iter()
        .any(|device| device.is_wireless() && device.is_ready()))
    .then(|| adb.advertised().unwrap_or_default());
    let selected = choose(&devices, requested.as_deref());
    let read = selected
        .as_deref()
        .filter(|serial| {
            devices
                .iter()
                .any(|device| device.serial == *serial && device.is_ready())
        })
        .map(|serial| adb.details(serial));
    let (details, details_error) = match read {
        Some(Ok(details)) => (Some(details), None),
        Some(Err(error)) => (None, Some(format!("{error:#}"))),
        None => (None, None),
    };
    Snapshot {
        requested,
        devices: Ok(devices),
        selected,
        details,
        details_error,
        advertised,
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
             192.168.1.20:5555        device product:seacliff model:Quest_Pro device:seacliff
             2G0YC0000000AB         device product:seacliff model:Quest_Pro device:seacliff
             1WMHH000000000         unauthorized usb:1-1
",
        );
        assert_eq!(choose(&devices, None).as_deref(), Some("2G0YC0000000AB"));
        assert_eq!(
            choose(&devices, Some("192.168.1.20:5555")).as_deref(),
            Some("192.168.1.20:5555")
        );
        assert_eq!(
            choose(&devices, Some("gone")).as_deref(),
            Some("2G0YC0000000AB")
        );
        assert_eq!(
            choose(&devices[3..], None).as_deref(),
            Some("1WMHH000000000")
        );
        assert_eq!(choose(&[], None), None);
    }
}

/// Headset firmware builds the per-eye gaze patch was made for. Its hook is
/// firmware-specific; see android/questpro-camera/README.md.
const EYE_TESTED_BUILD: &str = "51483620027600340";
const EYE_EXPERIMENTAL_BUILD: &str = "51503870024400340";

/// Whether per-eye gaze can work on the headset's firmware `build`.
fn eye_firmware_support(build: Option<&str>) -> (Tone, SharedString) {
    let (tone, text) = match build {
        None => (Tone::Off, t!("headset.firmware_unknown")),
        Some(build) if build.contains(EYE_TESTED_BUILD) => {
            (Tone::Good, t!("headset.firmware_tested"))
        }
        Some(build) if build.contains(EYE_EXPERIMENTAL_BUILD) => {
            (Tone::Waiting, t!("headset.firmware_untested"))
        }
        Some(_) => (Tone::Problem, t!("headset.firmware_unsupported")),
    };
    (tone, text.into())
}

#[cfg(test)]
mod eye_firmware_tests {
    use super::*;

    #[test]
    fn firmware_support_follows_the_known_builds() {
        assert_eq!(eye_firmware_support(None).0, Tone::Off);
        assert_eq!(
            eye_firmware_support(Some("51483620027600340")).0,
            Tone::Good
        );
        assert_eq!(
            eye_firmware_support(Some("user 51503870024400340 release-keys")).0,
            Tone::Waiting
        );
        assert_eq!(eye_firmware_support(Some("1")).0, Tone::Problem);
    }
}

/// How the headset looks to the page, for its setup list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetupDevice {
    Ready,
    Unauthorized,
    NotResponding,
}

struct SetupState {
    /// Whether adb was found; `None` while looking.
    adb: Option<bool>,
    device: Option<SetupDevice>,
    app_installed: Option<bool>,
    streaming: Option<bool>,
    received: bool,
}

struct SetupStep {
    label: SharedString,
    done: bool,
    /// What the step asks for, shown while it's to do.
    hint: SharedString,
}

/// The steps from nothing to VRFT receiving the headset's cameras. A step
/// counts as done when a later one is, since tracking can outlive the adb
/// connection.
fn setup_steps(state: &SetupState) -> Vec<SetupStep> {
    let receiving = state.received;
    let streaming = receiving || state.streaming == Some(true);
    let installed = streaming || state.app_installed == Some(true);
    let allowed = installed || state.device == Some(SetupDevice::Ready);
    let seen = allowed || state.device.is_some();
    let adb = seen || state.adb == Some(true);
    vec![
        SetupStep {
            label: t!("headset.step_download").into(),
            done: adb,
            hint: t!("headset.step_download_hint").into(),
        },
        SetupStep {
            label: t!("headset.step_connect").into(),
            done: seen,
            hint: t!("headset.step_connect_hint").into(),
        },
        SetupStep {
            label: t!("headset.step_allow").into(),
            done: allowed,
            hint: t!("headset.step_allow_hint").into(),
        },
        SetupStep {
            label: t!("headset.step_install").into(),
            done: installed,
            hint: t!("headset.step_install_hint").into(),
        },
        SetupStep {
            label: t!("headset.step_start").into(),
            done: streaming,
            hint: t!("headset.step_start_hint").into(),
        },
        SetupStep {
            label: t!("headset.step_receive").into(),
            done: receiving,
            hint: t!("headset.step_receive_hint").into(),
        },
    ]
}

#[cfg(test)]
mod setup_tests {
    use super::*;

    fn next(state: SetupState) -> Option<SharedString> {
        setup_steps(&state)
            .into_iter()
            .find(|step| !step.done)
            .map(|step| step.label)
    }

    #[test]
    fn the_next_step_follows_what_is_known() {
        let nothing = SetupState {
            adb: Some(false),
            device: None,
            app_installed: None,
            streaming: None,
            received: false,
        };
        assert!(next(nothing)
            .unwrap()
            .starts_with("Download the connection tool"));
        let unauthorized = SetupState {
            adb: Some(true),
            device: Some(SetupDevice::Unauthorized),
            app_installed: None,
            streaming: None,
            received: false,
        };
        assert!(next(unauthorized)
            .unwrap()
            .starts_with("Allow USB debugging"));
        let installed = SetupState {
            adb: Some(true),
            device: Some(SetupDevice::Ready),
            app_installed: Some(true),
            streaming: Some(false),
            received: false,
        };
        assert!(next(installed)
            .unwrap()
            .starts_with("Start the camera stream"));
        // Streaming over Wi-Fi with the cable out: everything is done.
        let receiving = SetupState {
            adb: Some(true),
            device: None,
            app_installed: None,
            streaming: None,
            received: true,
        };
        assert_eq!(next(receiving), None);
    }

    #[test]
    fn the_steps_with_controls_are_where_the_page_expects_them() {
        let steps = setup_steps(&SetupState {
            adb: Some(true),
            device: Some(SetupDevice::Ready),
            app_installed: Some(false),
            streaming: Some(false),
            received: false,
        });
        assert_eq!(&*steps[ALLOW_STEP].label, "Allow USB debugging");
        assert_eq!(
            &*steps[INSTALL_STEP].label,
            "Install the VRFaceTracking headset app"
        );
        assert_eq!(&*steps[START_STEP].label, "Start the camera stream");
        assert_eq!(&*steps[RECEIVE_STEP].label, "This PC receives the cameras");
    }
}
