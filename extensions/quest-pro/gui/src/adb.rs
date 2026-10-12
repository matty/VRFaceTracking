//! Talks to the headset through adb, Android's debug bridge: which adb to use,
//! running it, and reading what it prints. Nothing here touches GPUI, and
//! every call blocks, so the app makes them off the UI thread.
use anyhow::{bail, Context as _, Result};
use rust_i18n::t;
use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use vrft_gui_core::processes;

/// The headset app's Android package.
pub const PACKAGE: &str = "io.github.matty.vrft.questprocamera";
/// The headset app's screen, which also takes commands over adb.
const ACTIVITY: &str = "io.github.matty.vrft.questprocamera/.MainActivity";
/// The headset app's stream, which takes Start and Stop over adb without
/// bringing its screen to the front.
const SERVICE: &str = "io.github.matty.vrft.questprocamera/.CameraStreamService";
const START_ACTION: &str = "io.github.matty.vrft.questprocamera.START";
const STOP_ACTION: &str = "io.github.matty.vrft.questprocamera.STOP";
/// The port Quest's adb listens on after `adb tcpip`.
pub const WIRELESS_PORT: u16 = 5555;

#[cfg(windows)]
const EXE: &str = "adb.exe";
#[cfg(not(windows))]
const EXE: &str = "adb";

/// Google's Android SDK Platform-Tools for Windows, which contain adb.
const PLATFORM_TOOLS_URL: &str =
    "https://dl.google.com/android/repository/platform-tools-latest-windows.zip";
/// The terms that come with downloading Platform-Tools.
pub const PLATFORM_TOOLS_TERMS_URL: &str = "https://developer.android.com/studio/terms";
/// Downloaded Platform-Tools go in this folder, beside the app.
const PLATFORM_TOOLS_DIR: &str = "platform-tools";
/// The files adb needs from Platform-Tools. The rest, such as fastboot and
/// sqlite3, stay out.
const ADB_FILES: [&str; 3] = [EXE, "AdbWinApi.dll", "AdbWinUsbApi.dll"];
/// The licences for those files, kept with them when the archive has it.
const NOTICE_FILE: &str = "NOTICE.txt";
/// The archive is under 10 MB; anything far bigger isn't it.
const DOWNLOAD_LIMIT: u64 = 100 << 20;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// Most commands answer at once, or not at all.
const QUICK: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// Sending the APK takes seconds; the headset then checks it before replying.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(300);
/// How long the headset's adb takes to start listening on Wi-Fi.
const WIRELESS_START_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest the headset app takes to stop. Its own stop takes a second or two;
/// one from before it kept the per-eye model also put Meta's back, restarting
/// the headset's tracking service.
const STOP_TIMEOUT: Duration = Duration::from_secs(60);
/// The threads the headset app's service leaves behind as it stops: they stop
/// the camera relay and the eye trace (`CameraStreamService`'s `onDestroy`).
const STOPPING_THREADS: [&str; 2] = ["stop-relay", "stop-eye"];
/// A server that adb starts can inherit its pipes and hold them open, so once
/// adb itself has exited, wait only this long for them to close.
const PIPE_GRACE: Duration = Duration::from_millis(300);
const POLL: Duration = Duration::from_millis(20);
/// How much of the headset app's log the page shows.
const LOG_LINES: usize = 300;
/// Separates the parts of the details script's output.
const SEPARATOR: &str = "--vrft--";

/// Where the app found adb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The `VRFT_ADB` environment variable.
    Setting,
    /// In this app's folder, where downloading Platform-Tools puts it.
    App,
    /// The adb whose server is already running, perhaps for SideQuest or the
    /// Meta Quest Developer Hub. Ahead of the rest, because an adb with
    /// another protocol version would restart that server and drop their
    /// connection to the headset.
    Running,
    /// A development checkout's `.local/toolchain` Android SDK, or
    /// `android-tools/` beside the checkout.
    Workspace,
    Path,
    AndroidSdk,
    SideQuest,
    DeveloperHub,
}

impl Origin {
    pub fn label(self) -> Cow<'static, str> {
        match self {
            Origin::Setting => t!("adb.origin_setting"),
            Origin::App => t!("adb.origin_app"),
            Origin::Running => t!("adb.origin_running"),
            Origin::Workspace => t!("adb.origin_workspace"),
            Origin::Path => t!("adb.origin_path"),
            Origin::AndroidSdk => t!("adb.origin_android_sdk"),
            Origin::SideQuest => t!("adb.origin_sidequest"),
            Origin::DeveloperHub => t!("adb.origin_developer_hub"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adb {
    path: PathBuf,
    origin: Origin,
}

impl Adb {
    /// The adb to use, or `None` when this PC has none the app knows of.
    pub fn locate(app_dir: Option<&Path>) -> Option<Self> {
        candidates(app_dir)
            .into_iter()
            .find(|(path, _)| path.is_file())
            .map(|(path, origin)| Self { path, origin })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn origin(&self) -> Origin {
        self.origin
    }

    pub fn devices(&self) -> Result<Vec<Device>> {
        let output = self.run(["devices", "-l"], QUICK)?;
        output.check()?;
        Ok(parse_devices(&output.text))
    }

    /// Where devices on the network take adb over Wi-Fi, as adb's mDNS
    /// discovery finds them. A headset advertises this from `adb tcpip`
    /// until it restarts, but adb only connects by itself to devices paired
    /// for wireless debugging, not to these.
    pub fn advertised(&self) -> Result<Vec<SocketAddr>> {
        let output = self.run(["mdns", "services"], QUICK)?;
        output.check()?;
        Ok(parse_mdns_services(&output.text))
    }

    /// The app, battery and network state of one headset, read in one call.
    pub fn details(&self, serial: &str) -> Result<Details> {
        let script = format!(
            "dumpsys package {PACKAGE} | grep -E 'versionCode=|versionName='; echo {SEPARATOR}; \
             {streaming}; echo {SEPARATOR}; \
             dumpsys battery | grep -E '^ *(level|status):'; echo {SEPARATOR}; \
             ip -f inet addr show wlan0; echo {SEPARATOR}; \
             getprop ro.build.version.incremental",
            streaming = streaming_script(),
        );
        Ok(parse_details(&self.shell(serial, &script)?))
    }

    /// Switches a headset connected over USB to take adb over Wi-Fi as well,
    /// then connects to it there. Returns the new connection's serial. The
    /// headset keeps listening until it restarts.
    pub fn enable_wireless(&self, serial: &str, address: Ipv4Addr) -> Result<String> {
        let port = WIRELESS_PORT.to_string();
        self.run(["-s", serial, "tcpip", port.as_str()], QUICK)?
            .check()?;
        let target = SocketAddr::from((address, WIRELESS_PORT)).to_string();
        let began = Instant::now();
        loop {
            // The headset's adb restarts to listen on the network.
            thread::sleep(Duration::from_secs(1));
            match self.connect(&target) {
                Ok(serial) => return Ok(serial),
                // Connecting again would only ask the headset again.
                Err(error) if error.is::<NotAllowed>() => return Err(error),
                Err(error) if began.elapsed() > WIRELESS_START_TIMEOUT => return Err(error),
                Err(_) => {}
            }
        }
    }

    /// Connects to a headset that takes adb over Wi-Fi, at `ip:port`, and
    /// returns the connection's serial. A connection adb keeps but can't use,
    /// such as one the headset hasn't allowed, is made again: the headset
    /// asks to allow debugging only as a connection is made, and one that
    /// was asleep then never shows the question.
    pub fn connect(&self, target: &str) -> Result<String> {
        let unusable = self.devices().is_ok_and(|devices| {
            devices
                .iter()
                .any(|device| device.serial == target && !device.is_ready())
        });
        if unusable {
            self.disconnect(target)?;
        }
        let output = self.run(["connect", target], CONNECT_TIMEOUT)?;
        parse_connect(target, &output.text)
    }

    pub fn disconnect(&self, serial: &str) -> Result<()> {
        self.run(["disconnect", serial], QUICK)?.check()
    }

    /// Installs `apk` over whatever version the headset has, keeping the
    /// app's settings.
    pub fn install(&self, serial: &str, apk: &Path) -> Result<(), InstallError> {
        let args: [&OsStr; 5] = [
            "-s".as_ref(),
            serial.as_ref(),
            "install".as_ref(),
            "-r".as_ref(),
            apk.as_os_str(),
        ];
        let output = self
            .run(args, INSTALL_TIMEOUT)
            .map_err(|error| InstallError::Failed(format!("{error:#}")))?;
        parse_install(output.success, &output.text)
    }

    /// Removes the headset app and its settings.
    pub fn uninstall(&self, serial: &str) -> Result<()> {
        let output = self.run(["-s", serial, "uninstall", PACKAGE], CONNECT_TIMEOUT)?;
        if output.text.lines().any(|line| line.trim() == "Success") {
            Ok(())
        } else {
            bail!(reason(&output.text))
        }
    }

    /// Opens the headset app on the headset with `extras`, which it treats as
    /// its own controls: settings it saves, and presses of Start or Stop.
    pub fn open_app(&self, serial: &str, extras: &[(&str, bool)]) -> Result<()> {
        // Without single-top, Horizon OS only brings an open app to the front
        // ("task to front") and drops the extras, so Start, Stop and settings
        // went nowhere while the app's window was open.
        let mut command = format!("am start --activity-single-top -n {ACTIVITY}");
        for (key, value) in extras {
            command.push_str(&format!(" --ez {key} {value}"));
        }
        let text = self.shell(serial, &command)?;
        if text.contains("does not exist") {
            bail!(t!("adb.app_not_installed"));
        }
        if let Some(error) = text.lines().find(|line| line.starts_with("Error:")) {
            bail!(error.trim_start_matches("Error:").trim().to_string());
        }
        Ok(())
    }

    /// Starts the headset app's stream with `extras` saved as its settings
    /// first, leaving whatever is in front, such as Virtual Desktop, there.
    pub fn start_stream(&self, serial: &str, extras: &[(&str, bool)]) -> Result<()> {
        let mut command = format!("am start-foreground-service -n {SERVICE} -a {START_ACTION}");
        for (key, value) in extras {
            command.push_str(&format!(" --ez {key} {value}"));
        }
        match self.service_command(serial, &command)? {
            ServiceReply::Done => Ok(()),
            // An app from before its stream took commands from adb.
            ServiceReply::Private => {
                let mut extras = extras.to_vec();
                extras.push(("start_probe", true));
                self.open_app(serial, &extras)
            }
            ServiceReply::Background(error) => bail!(error),
        }
    }

    /// Whether the headset app's camera stream service is running.
    pub fn streaming(&self, serial: &str) -> Result<bool> {
        Ok(parse_count(&self.shell(serial, &streaming_script())?) > 0)
    }

    /// Presses Stop in the headset app, then waits until it has finished
    /// stopping (an app from before it kept the per-eye model also puts Meta's
    /// eye model back). Replacing the app before then would kill it half way
    /// through.
    pub fn stop_stream(&self, serial: &str) -> Result<()> {
        // A plain service start, since a foreground one must go foreground.
        // Android turns it away only when no stream runs, so nothing is lost.
        let command = format!("am startservice -n {SERVICE} -a {STOP_ACTION}");
        if self.service_command(serial, &command)? == ServiceReply::Private {
            self.open_app(serial, &[("stop_probe", true)])?;
        }
        let began = Instant::now();
        loop {
            thread::sleep(Duration::from_millis(500));
            // The service is gone before its clean-up threads start, so give
            // them a moment to appear.
            if began.elapsed() >= Duration::from_secs(2)
                && !self.streaming(serial)?
                && !self.stopping(serial)?
            {
                return Ok(());
            }
            if began.elapsed() > STOP_TIMEOUT {
                bail!(t!("adb.app_still_stopping"));
            }
        }
    }

    /// Whether the headset app is still cleaning up after its stream.
    fn stopping(&self, serial: &str) -> Result<bool> {
        // `ps -T` names each thread of the app's process.
        let text = self.shell(
            serial,
            &format!("for pid in $(pidof {PACKAGE}); do ps -T -p $pid -o CMD; done"),
        )?;
        Ok(text
            .lines()
            .any(|line| STOPPING_THREADS.contains(&line.trim())))
    }

    /// The headset app's recent log, with any crash.
    pub fn app_log(&self, serial: &str) -> Result<String> {
        // logcat's own `-t` counts lines before the tag filter, so it would
        // mostly take other apps' lines, which the filter then drops.
        let output = self.run(
            [
                "-s",
                serial,
                "logcat",
                "-d",
                "-v",
                "time",
                "-s",
                "VRFTCamera:V",
                "AndroidRuntime:E",
            ],
            QUICK,
        )?;
        output.check()?;
        Ok(last_log_lines(&output.text))
    }

    /// Runs an `am` command that starts the headset app's service. `am`
    /// fails for a refused start, so its words are what tell them apart.
    fn service_command(&self, serial: &str, command: &str) -> Result<ServiceReply> {
        parse_service_reply(&self.shell(serial, &format!("{command} 2>&1 || true"))?)
    }

    fn shell(&self, serial: &str, command: &str) -> Result<String> {
        let output = self.run(["-s", serial, "shell", command], QUICK)?;
        output.check()?;
        Ok(output.text)
    }

    fn run<I, S>(&self, args: I, timeout: Duration) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<OsString> = args
            .into_iter()
            .map(|arg| arg.as_ref().to_os_string())
            .collect();
        let mut command = Command::new(&self.path);
        processes::hidden(&mut command)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .with_context(|| t!("adb.couldnt_run", path = self.path.display()))?;
        let stdout = Collector::new(child.stdout.take());
        let stderr = Collector::new(child.stderr.take());
        let deadline = Instant::now() + timeout;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                let command = args
                    .iter()
                    .map(|arg| arg.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(" ");
                bail!(t!(
                    "adb.command_timed_out",
                    command = command,
                    seconds = timeout.as_secs()
                ));
            }
            thread::sleep(POLL);
        };
        let mut text = stdout.finish();
        text.push_str(&stderr.finish());
        Ok(Output {
            success: status.success(),
            text: text.replace("\r\n", "\n"),
        })
    }
}

/// The last [`LOG_LINES`] of logcat's output, without its buffer headers.
fn last_log_lines(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.starts_with("--------- "))
        .collect();
    lines[lines.len().saturating_sub(LOG_LINES)..].join("\n")
}

/// Counts the headset app's running stream services.
fn streaming_script() -> String {
    format!("dumpsys activity services {PACKAGE} | grep -c 'ServiceRecord{{' || true")
}

/// Every place adb might be, best first.
fn candidates(app_dir: Option<&Path>) -> Vec<(PathBuf, Origin)> {
    let mut found = Vec::new();
    if let Some(path) = std::env::var_os("VRFT_ADB") {
        found.push((PathBuf::from(path), Origin::Setting));
    }
    // A running adb first, even before this app's own copy: another
    // version would restart its server and drop SideQuest's connection.
    for pid in processes::find(EXE) {
        if let Some(path) = processes::executable(pid) {
            found.push((path, Origin::Running));
        }
    }
    if let Some(app_dir) = app_dir {
        found.push((platform_tools_dir(app_dir).join(EXE), Origin::App));
        found.push((app_dir.join(EXE), Origin::App));
    }
    if let Some(checkout) = app_dir.and_then(vrft_gui_core::paths::checkout_root) {
        found.push((
            checkout
                .join(".local/toolchain/android-sdk")
                .join(PLATFORM_TOOLS_DIR)
                .join(EXE),
            Origin::Workspace,
        ));
        if let Some(workspace) = checkout.parent() {
            found.push((
                workspace
                    .join("android-tools")
                    .join(PLATFORM_TOOLS_DIR)
                    .join(EXE),
                Origin::Workspace,
            ));
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            found.push((dir.join(EXE), Origin::Path));
        }
    }
    let env_dir = |name: &str| std::env::var_os(name).map(PathBuf::from);
    for sdk in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Some(dir) = env_dir(sdk) {
            found.push((dir.join(PLATFORM_TOOLS_DIR).join(EXE), Origin::AndroidSdk));
        }
    }
    if let Some(dir) = env_dir("LOCALAPPDATA") {
        found.push((
            dir.join("Android/Sdk").join(PLATFORM_TOOLS_DIR).join(EXE),
            Origin::AndroidSdk,
        ));
    }
    if let Some(dir) = env_dir("APPDATA") {
        found.push((
            dir.join("SideQuest").join(PLATFORM_TOOLS_DIR).join(EXE),
            Origin::SideQuest,
        ));
    }
    if let Some(dir) = env_dir("ProgramFiles") {
        found.push((
            dir.join("Meta Quest Developer Hub/resources/bin").join(EXE),
            Origin::DeveloperHub,
        ));
    }
    found
}

/// Where downloaded Platform-Tools go: a folder in the app's folder.
pub fn platform_tools_dir(app_dir: &Path) -> PathBuf {
    app_dir.join(PLATFORM_TOOLS_DIR)
}

/// Downloads Google's Platform-Tools and puts the files adb needs in the
/// app's folder, replacing any earlier copy, then returns that adb.
pub fn download_platform_tools(app_dir: &Path) -> Result<Adb> {
    // Download and unpack in a staging folder first, so a failure leaves an
    // earlier copy as it was.
    let staging = app_dir.join("platform-tools-download");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)
        .with_context(|| t!("adb.couldnt_write", path = app_dir.display()))?;
    let destination = platform_tools_dir(app_dir);
    let result = download_into(&staging)
        .and_then(|unpacked| install_platform_tools(&unpacked, &destination));
    let _ = fs::remove_dir_all(&staging);
    result?;
    Ok(Adb {
        path: destination.join(EXE),
        origin: Origin::App,
    })
}

/// Downloads the Platform-Tools archive into `staging` and unpacks it there,
/// returning the unpacked folder.
fn download_into(staging: &Path) -> Result<PathBuf> {
    let archive = staging.join("platform-tools.zip");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(DOWNLOAD_TIMEOUT))
        .build()
        .into();
    let mut response = agent
        .get(PLATFORM_TOOLS_URL)
        .call()
        .context(t!("adb.couldnt_download"))?;
    let mut file = fs::File::create(&archive)
        .with_context(|| t!("adb.couldnt_save_download", path = staging.display()))?;
    io::copy(
        &mut response
            .body_mut()
            .with_config()
            .limit(DOWNLOAD_LIMIT)
            .reader(),
        &mut file,
    )
    .context(t!("adb.download_stopped"))?;
    drop(file);
    unpack(&archive, staging)?;
    Ok(staging.join(PLATFORM_TOOLS_DIR))
}

/// Moves the files adb needs, and their licences, from the unpacked archive
/// into `destination`.
fn install_platform_tools(unpacked: &Path, destination: &Path) -> Result<()> {
    if let Some(missing) = ADB_FILES.iter().find(|file| !unpacked.join(file).is_file()) {
        bail!(t!("adb.download_missing_file", file = missing));
    }
    fs::create_dir_all(destination)
        .with_context(|| t!("adb.couldnt_create", path = destination.display()))?;
    let notice = Some(NOTICE_FILE).filter(|file| unpacked.join(file).is_file());
    for file in ADB_FILES.into_iter().chain(notice) {
        fs::rename(unpacked.join(file), destination.join(file)).with_context(|| {
            t!(
                "adb.couldnt_replace",
                file = file,
                path = destination.display()
            )
        })?;
    }
    Ok(())
}

/// Unpacks a zip with the `tar` that Windows has had since Windows 10.
fn unpack(archive: &Path, into: &Path) -> Result<()> {
    let windows =
        std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    let mut command = Command::new(windows.join("System32").join("tar.exe"));
    let output = processes::hidden(&mut command)
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .stdin(Stdio::null())
        .output()
        .context(t!("adb.couldnt_unpack"))?;
    if !output.status.success() {
        bail!(t!(
            "adb.couldnt_unpack_because",
            reason = String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

/// What an adb command printed, standard output then standard error.
struct Output {
    success: bool,
    text: String,
}

impl Output {
    fn check(&self) -> Result<()> {
        if self.success {
            Ok(())
        } else {
            bail!(reason(&self.text))
        }
    }
}

/// Reads a pipe on its own thread.
struct Collector {
    buffer: Arc<Mutex<Vec<u8>>>,
    done: mpsc::Receiver<()>,
}

impl Collector {
    fn new(pipe: Option<impl Read + Send + 'static>) -> Self {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let (finished, done) = mpsc::channel();
        if let Some(mut pipe) = pipe {
            let buffer = buffer.clone();
            thread::spawn(move || {
                let mut chunk = [0; 4096];
                loop {
                    match pipe.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => buffer.lock().unwrap().extend_from_slice(&chunk[..read]),
                    }
                }
                let _ = finished.send(());
            });
        }
        Self { buffer, done }
    }

    fn finish(self) -> String {
        let _ = self.done.recv_timeout(PIPE_GRACE);
        let buffer = self.buffer.lock().unwrap();
        String::from_utf8_lossy(&buffer).into_owned()
    }
}

/// adb's explanation of a failure: its last line that says something.
fn reason(text: &str) -> String {
    let Some(line) = text
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty() && !line.starts_with('*'))
    else {
        return t!("adb.failed_silently").into();
    };
    let line = line
        .strip_prefix("error:")
        .or_else(|| line.strip_prefix("adb:"))
        .unwrap_or(line)
        .trim();
    match line {
        "no devices/emulators found" => t!("adb.headset_not_connected").into(),
        "device offline" => t!("adb.headset_offline").into(),
        line if line.contains("unauthorized") => t!("adb.headset_unauthorized").into(),
        line if line.starts_with("device '") && line.ends_with("' not found") => {
            t!("adb.headset_disconnected").into()
        }
        line => capitalize(line),
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub serial: String,
    pub state: DeviceState,
    /// `Quest_Pro`, as adb reports it.
    model: Option<String>,
    /// `seacliff` for a Quest Pro.
    product: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceState {
    Ready,
    /// The headset hasn't allowed this computer to debug it yet.
    Unauthorized,
    Offline,
    Other(String),
}

impl Device {
    pub fn name(&self) -> String {
        match &self.model {
            Some(model) => model.replace('_', " "),
            None => t!("adb.android_device").into(),
        }
    }

    pub fn is_ready(&self) -> bool {
        self.state == DeviceState::Ready
    }

    /// Where the headset is on the network, when adb reaches it over Wi-Fi.
    pub fn wireless_address(&self) -> Option<SocketAddr> {
        self.serial.parse().ok()
    }

    pub fn is_wireless(&self) -> bool {
        self.wireless_address().is_some() || self.serial.contains("._adb-tls-connect.")
    }

    pub fn is_quest_pro(&self) -> bool {
        self.product.as_deref() == Some("seacliff")
    }
}

pub(crate) fn parse_devices(text: &str) -> Vec<Device> {
    text.lines()
        .filter(|line| !line.starts_with("List of devices") && !line.starts_with('*'))
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let serial = fields.next()?.to_string();
            let state = match fields.next()? {
                "device" => DeviceState::Ready,
                "unauthorized" => DeviceState::Unauthorized,
                "offline" => DeviceState::Offline,
                other => DeviceState::Other(other.to_string()),
            };
            let mut device = Device {
                serial,
                state,
                model: None,
                product: None,
            };
            for field in fields {
                match field.split_once(':') {
                    Some(("model", model)) => device.model = Some(model.to_string()),
                    Some(("product", product)) => device.product = Some(product.to_string()),
                    _ => {}
                }
            }
            Some(device)
        })
        .collect()
}

/// The IPv4 addresses of `_adb._tcp` services in `adb mdns services`, such
/// as `adb-2G0YC0000000AB  _adb._tcp  192.168.1.20:5555`. Older adb writes the
/// service type with a trailing dot.
pub(crate) fn parse_mdns_services(text: &str) -> Vec<SocketAddr> {
    let mut found = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if !fields
            .iter()
            .any(|field| field.trim_end_matches('.') == "_adb._tcp")
        {
            continue;
        }
        let address = fields
            .last()
            .and_then(|field| field.parse::<SocketAddr>().ok());
        if let Some(address) = address.filter(SocketAddr::is_ipv4) {
            if !found.contains(&address) {
                found.push(address);
            }
        }
    }
    found
}

/// A headset's state that the app shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Details {
    pub app: Option<InstalledApp>,
    /// Whether the headset app's camera stream is running.
    pub streaming: bool,
    pub battery: Option<Battery>,
    pub wifi_address: Option<Ipv4Addr>,
    /// The firmware build, such as `51503870024400340`.
    pub build: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledApp {
    pub version_name: String,
    pub version_code: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Battery {
    pub percent: u8,
    pub charging: bool,
}

fn parse_details(text: &str) -> Details {
    let mut parts = text.split(SEPARATOR).map(str::trim);
    let mut part = || parts.next().unwrap_or_default();
    let (package, services, battery, network, build) = (part(), part(), part(), part(), part());
    Details {
        app: parse_installed(package),
        streaming: parse_count(services) > 0,
        battery: parse_battery(battery),
        wifi_address: network.lines().find_map(|line| {
            let address = line.trim().strip_prefix("inet ")?;
            address.split(['/', ' ']).next()?.parse().ok()
        }),
        build: Some(build.to_string()).filter(|build| !build.is_empty()),
    }
}

fn parse_installed(text: &str) -> Option<InstalledApp> {
    let value = |key: &str| {
        text.split_whitespace()
            .find_map(|field| field.strip_prefix(key))
            .map(str::to_string)
    };
    Some(InstalledApp {
        version_code: value("versionCode=")?.parse().ok()?,
        version_name: value("versionName=")?,
    })
}

fn parse_battery(text: &str) -> Option<Battery> {
    let value = |key: &str| {
        text.lines()
            .find_map(|line| line.trim().strip_prefix(key)?.trim().parse::<u8>().ok())
    };
    // Android's BatteryManager.BATTERY_STATUS_CHARGING.
    const CHARGING: u8 = 2;
    Some(Battery {
        percent: value("level:")?,
        charging: value("status:") == Some(CHARGING),
    })
}

fn parse_count(text: &str) -> u32 {
    text.trim().parse().unwrap_or(0)
}

/// What `am` said to a start of the headset app's service.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ServiceReply {
    Done,
    /// The service is private to the app, as before it took commands.
    Private,
    /// Android won't start a service in an app that runs nothing, with why.
    Background(String),
}

fn parse_service_reply(text: &str) -> Result<ServiceReply> {
    let Some(error) = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("Error:"))
        .map(str::trim)
    else {
        return Ok(ServiceReply::Done);
    };
    if error.starts_with("Requires permission") {
        Ok(ServiceReply::Private)
    } else if error.starts_with("Not found") {
        bail!(t!("adb.app_not_installed"))
    } else if error.contains("app is in background") {
        Ok(ServiceReply::Background(error.to_string()))
    } else {
        bail!(error.to_string())
    }
}

fn parse_connect(target: &str, text: &str) -> Result<String> {
    // Connected, but waiting for the wearer to allow it.
    if text.contains("failed to authenticate") {
        return Err(NotAllowed(target.to_string()).into());
    }
    // "connected to" or "already connected to".
    if text.contains("connected to") {
        return Ok(target.to_string());
    }
    let reason = reason(text);
    if reason.contains("refused") || reason.contains("10061") {
        bail!(t!("adb.connection_refused", target = target));
    }
    if reason.contains("10060") || reason.contains("timed out") {
        bail!(t!("adb.no_answer", target = target));
    }
    bail!(reason)
}

/// The `ip:port` adb connects to for an address typed with or without its
/// port.
pub fn wireless_target(input: &str) -> Option<String> {
    let input = input.trim();
    if let Ok(address) = input.parse::<SocketAddr>() {
        return Some(address.to_string());
    }
    let ip: IpAddr = input.parse().ok()?;
    Some(SocketAddr::new(ip, WIRELESS_PORT).to_string())
}

/// adb reached the headset at this `ip:port`, but the headset hasn't allowed
/// this computer to debug it, and asks in the headset. Once Allow is chosen,
/// it takes the next connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotAllowed(pub String);

impl fmt::Display for NotAllowed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&t!("adb.waiting_for_allow", target = self.0))
    }
}

impl std::error::Error for NotAllowed {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallError {
    /// The installed app is signed with another key: a release over a
    /// development build, or the other way round.
    DifferentSigner,
    /// The installed app is newer.
    Downgrade,
    Failed(String),
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InstallError::DifferentSigner => f.write_str(&t!("adb.install_different_signer")),
            InstallError::Downgrade => f.write_str(&t!("adb.install_downgrade")),
            InstallError::Failed(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for InstallError {}

fn parse_install(success: bool, text: &str) -> Result<(), InstallError> {
    if success && text.lines().any(|line| line.trim() == "Success") {
        return Ok(());
    }
    let code = text
        .split_once("Failure [")
        .and_then(|(_, rest)| rest.split([']', ':', ' ']).next());
    match code {
        Some("INSTALL_FAILED_UPDATE_INCOMPATIBLE") => Err(InstallError::DifferentSigner),
        Some("INSTALL_FAILED_VERSION_DOWNGRADE") => Err(InstallError::Downgrade),
        _ => Err(InstallError::Failed(reason(text))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_devices_in_every_state() {
        let text = "* daemon not running; starting now at tcp:5037\n\
                    * daemon started successfully\n\
                    List of devices attached\n\
                    2G0YC0000000AB         device product:seacliff model:Quest_Pro device:seacliff transport_id:1\n\
                    192.168.1.20:5555        device product:seacliff model:Quest_Pro device:seacliff transport_id:2\n\
                    1WMHH000000000         unauthorized usb:1-1 transport_id:3\n\
                    emulator-5554          offline transport_id:4\n\n";
        let devices = parse_devices(text);
        assert_eq!(devices.len(), 4);
        assert_eq!(devices[0].name(), "Quest Pro");
        assert!(devices[0].is_ready() && devices[0].is_quest_pro());
        assert!(!devices[0].is_wireless());
        assert_eq!(
            devices[1].wireless_address(),
            Some("192.168.1.20:5555".parse().unwrap())
        );
        assert_eq!(devices[2].state, DeviceState::Unauthorized);
        assert_eq!(devices[2].name(), "Android device");
        assert_eq!(devices[3].state, DeviceState::Offline);
    }

    #[test]
    fn reads_the_details_script() {
        let text = "    versionCode=2026090000 minSdk=29 targetSdk=34\n    versionName=2026.9.0\n\
                    --vrft--\n1\n--vrft--\n  status: 2\n  level: 89\n--vrft--\n\
                    9: wlan0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc pfifo_fast state UP group default qlen 1000\n\
                    \x20   inet 192.168.1.20/24 brd 192.168.1.255 scope global wlan0\n\
                    \x20      valid_lft forever preferred_lft forever\n\
                    --vrft--\n51503870024400340\n";
        let details = parse_details(text);
        assert_eq!(
            details.app,
            Some(InstalledApp {
                version_name: "2026.9.0".into(),
                version_code: 2026090000
            })
        );
        assert!(details.streaming);
        assert_eq!(
            details.battery,
            Some(Battery {
                percent: 89,
                charging: true
            })
        );
        assert_eq!(details.wifi_address, Some(Ipv4Addr::new(192, 168, 1, 20)));
        assert_eq!(details.build.as_deref(), Some("51503870024400340"));
    }

    #[test]
    fn details_without_the_app_or_wifi() {
        let details = parse_details("--vrft--\n0\n--vrft--\n  level: 40\n--vrft--\n--vrft--\n");
        assert_eq!(details.app, None);
        assert!(!details.streaming);
        assert_eq!(
            details.battery,
            Some(Battery {
                percent: 40,
                charging: false
            })
        );
        assert_eq!(details.wifi_address, None);
        assert_eq!(details.build, None);
    }

    #[test]
    fn install_failures_say_why() {
        assert_eq!(
            parse_install(true, "Performing Streamed Install\nSuccess\n"),
            Ok(())
        );
        assert_eq!(
            parse_install(
                false,
                "Performing Streamed Install\nadb: failed to install app.apk: Failure [INSTALL_FAILED_UPDATE_INCOMPATIBLE: \
                 Existing package io.github.matty.vrft.questprocamera signatures do not match newer version; ignoring!]\n"
            ),
            Err(InstallError::DifferentSigner)
        );
        assert_eq!(
            parse_install(false, "Failure [INSTALL_FAILED_VERSION_DOWNGRADE]\n"),
            Err(InstallError::Downgrade)
        );
        assert_eq!(
            parse_install(false, "adb: device offline\n"),
            Err(InstallError::Failed(
                "The headset isn't responding. Unplug it and plug it back in.".into()
            ))
        );
    }

    #[test]
    fn connect_explains_a_refusal() {
        let target = "192.168.1.20:5555";
        assert_eq!(
            parse_connect(target, "connected to 192.168.1.20:5555\n").unwrap(),
            target
        );
        assert_eq!(
            parse_connect(target, "already connected to 192.168.1.20:5555\n").unwrap(),
            target
        );
        let waiting =
            parse_connect(target, "failed to authenticate to 192.168.1.20:5555\n").unwrap_err();
        assert_eq!(
            waiting.downcast_ref::<NotAllowed>(),
            Some(&NotAllowed(target.to_string()))
        );
        assert!(waiting.to_string().contains("allow USB debugging"));
        let refused = parse_connect(
            target,
            "failed to connect to '192.168.1.20:5555': Connection refused\n",
        )
        .unwrap_err()
        .to_string();
        assert!(refused.contains("USB cable"), "{refused}");
    }

    #[test]
    fn log_keeps_the_last_lines_without_buffer_headers() {
        let mut text = String::from("--------- beginning of main\n");
        for line in 0..LOG_LINES + 5 {
            text.push_str(&format!(
                "09-25 18:14:25.664 I/VRFTCamera(31111): line {line}\n"
            ));
        }
        let log = last_log_lines(&text);
        assert_eq!(log.lines().count(), LOG_LINES);
        assert!(log.starts_with("09-25 18:14:25.664 I/VRFTCamera(31111): line 5\n"));
        assert_eq!(last_log_lines("--------- beginning of crash\n"), "");
    }

    #[test]
    fn installs_only_what_adb_needs_and_its_licences() {
        let root = std::env::temp_dir().join(format!("vrft-adb-test-{}", std::process::id()));
        let unpacked = root.join("staging/platform-tools");
        let destination = root.join("app/platform-tools");
        fs::create_dir_all(&unpacked).unwrap();
        for file in ADB_FILES.into_iter().chain([NOTICE_FILE, "fastboot.exe"]) {
            fs::write(unpacked.join(file), file).unwrap();
        }
        // An earlier copy is replaced.
        fs::create_dir_all(&destination).unwrap();
        fs::write(destination.join(EXE), "old").unwrap();

        install_platform_tools(&unpacked, &destination).unwrap();
        let mut installed: Vec<String> = fs::read_dir(&destination)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        installed.sort();
        let mut expected: Vec<String> = ADB_FILES
            .into_iter()
            .chain([NOTICE_FILE])
            .map(str::to_string)
            .collect();
        expected.sort();
        assert_eq!(installed, expected);
        assert_eq!(fs::read_to_string(destination.join(EXE)).unwrap(), EXE);

        // Installing moved the files, so stage an archive without one DLL.
        for file in [EXE, "AdbWinApi.dll"] {
            fs::write(unpacked.join(file), file).unwrap();
        }
        let error = install_platform_tools(&unpacked, &destination).unwrap_err();
        assert_eq!(
            error.to_string(),
            "The download didn't contain AdbWinUsbApi.dll."
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn wireless_target_adds_the_default_port() {
        assert_eq!(
            wireless_target(" 192.168.1.20 ").as_deref(),
            Some("192.168.1.20:5555")
        );
        assert_eq!(
            wireless_target("192.168.1.20:5037").as_deref(),
            Some("192.168.1.20:5037")
        );
        assert_eq!(wireless_target("quest.local"), None);
        assert_eq!(wireless_target(""), None);
    }

    #[test]
    fn finds_headsets_advertising_adb_over_wifi() {
        let services = parse_mdns_services(
            "List of discovered mdns services\n\
             adb-2G0YC0000000AB\t_adb._tcp\t192.168.1.20:5555\n\
             adb-1WMHH000000000\t_adb._tcp.\t192.168.1.21:5555\n\
             adb-1WMHH000000000\t_adb._tcp.\t192.168.1.21:5555\n\
             adb-R5CT00000000-AbCdEf\t_adb-tls-connect._tcp\t192.168.1.22:37123\n\
             adb-R5CT00000000-AbCdEf\t_adb-tls-pairing._tcp\t192.168.1.22:41234\n\
             adb-2G0YC0000000AB\t_adb._tcp\t[fe80::1]:5555\n",
        );
        assert_eq!(
            services,
            [
                "192.168.1.20:5555".parse::<SocketAddr>().unwrap(),
                "192.168.1.21:5555".parse().unwrap(),
            ]
        );
        assert!(parse_mdns_services("List of discovered mdns services\n").is_empty());
    }

    #[test]
    fn reads_what_am_says_to_a_service_start() {
        let started = "Starting service: Intent { act=io.github.matty.vrft.questprocamera.START \
                       cmp=io.github.matty.vrft.questprocamera/.CameraStreamService (has extras) }\n";
        assert_eq!(parse_service_reply(started).unwrap(), ServiceReply::Done);
        let private = "Starting service: Intent { ... }\n\
                       Error: Requires permission not exported from uid 10123\n";
        assert_eq!(parse_service_reply(private).unwrap(), ServiceReply::Private);
        let background = "Starting service: Intent { ... }\n\
                          Error: app is in background uid UidRecord{1234 u0a123 CEM  idle}\n";
        assert!(matches!(
            parse_service_reply(background).unwrap(),
            ServiceReply::Background(_)
        ));
        let missing = "Starting service: Intent { ... }\nError: Not found; no service started.\n";
        assert!(parse_service_reply(missing).is_err());
        assert!(parse_service_reply("Error: something else\n").is_err());
    }

    #[test]
    fn reason_takes_the_last_line_and_explains_common_errors() {
        assert_eq!(
            reason("error: no devices/emulators found\n"),
            "The headset isn't connected."
        );
        assert_eq!(
            reason("error: device '2G0YC0000000AB' not found\n"),
            "The headset has disconnected."
        );
        assert_eq!(reason("something\nerror: closed\n\n"), "Closed");
        assert_eq!(reason(""), "The connection tool failed without saying why");
    }
}
