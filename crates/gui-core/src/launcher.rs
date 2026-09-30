//! Starts and stops the daemon, `vrft_d`, for the app.
//!
//! The app needs the daemon for everything it shows, so it starts the daemon
//! when it opens, and offers to start it again whenever it isn't running. The
//! daemon runs without a console window, with its output in `vrft_d.log` in
//! the folder it runs in. A daemon the app started stops when the app
//! closes, and by itself if the app crashes or is ended; one that was
//! already running, such as one started from a console, is left alone.
//!
//! The app holds the daemon it uses by its process handle: the one it
//! started, or the one that answers, by the process id it reports. Stopping
//! or ending the daemon only ever reaches that process, never another
//! `vrft_d.exe` such as a tongue training run.
use crate::client::DaemonClient;
use crate::live::DaemonState;
use crate::processes::Process;
use crate::summary::{Connection, Launch, Tone};
use crate::widgets::Notice;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{h_flex, v_flex, Disableable as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Context, Entity, IntoElement, ParentElement, RenderOnce, SharedString, Styled,
    Subscription, Task, Window,
};
use rust_i18n::t;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
use vrft_protocol::{DAEMON_INSTANCE, OWNER_PID_ARG};

const DAEMON_EXE: &str = "vrft_d.exe";
const LOG_FILE: &str = "vrft_d.log";
/// Loading a .NET module or the tongue runtime can take a while, but not this
/// long.
const START_TIMEOUT: Duration = Duration::from_secs(30);
/// The daemon stops within a second or two once asked, and ends itself if
/// stopping takes more than a few.
const STOP_TIMEOUT: Duration = Duration::from_secs(15);
const CHECK_INTERVAL: Duration = Duration::from_millis(250);
/// How long a stopped daemon's process may be gone before the app's status
/// polling notices; stopping is done then even if it hasn't.
const GONE_GRACE: Duration = Duration::from_secs(3);
/// How long closing the app waits for its daemon to stop by itself before
/// ending it. Waiting at all lets an update replace `vrft_d.exe` once the app
/// has closed.
const QUIT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long an ended daemon takes to be gone.
const END_TIMEOUT: Duration = Duration::from_secs(5);

/// The daemon process the app is using.
struct DaemonProcess {
    process: Arc<Process>,
    /// This app started it, so stops it when it closes.
    started_here: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LaunchState {
    Idle,
    Starting,
    Stopping,
    Failed(SharedString),
}

pub struct Launcher {
    daemon: Entity<DaemonState>,
    /// Kept so the daemon can be asked to stop when this is dropped, when
    /// `daemon` may already be gone.
    client: Arc<DaemonClient>,
    /// `vrft_d.exe` beside this app, if it is there.
    executable: Option<PathBuf>,
    state: LaunchState,
    /// The daemon this app started or answers it, while it runs.
    process: Option<DaemonProcess>,
    /// A daemon's process id that couldn't be opened, so it isn't tried again
    /// on every status.
    unopened: Option<u32>,
    /// Start the daemon again once it has stopped.
    restart: bool,
    /// The failure shown is from starting, so it clears once the daemon
    /// answers after all, such as a slow module that outlasted the timeout.
    start_failed: bool,
    /// The user stopped the daemon, and it hasn't been started since.
    stopped_by_user: bool,
    /// The daemon the app uses is running but not answering, so the app
    /// offers to end it.
    stuck: bool,
    _task: Option<Task<()>>,
    _online: Subscription,
}

impl Launcher {
    pub fn new(daemon: Entity<DaemonState>, cx: &mut Context<Self>) -> Self {
        let executable = std::env::current_exe()
            .ok()
            .map(|app| app.with_file_name(DAEMON_EXE))
            .filter(|path| path.is_file());
        let online = cx.observe(&daemon, |launcher, _, cx| {
            launcher.follow_answering_daemon(cx);
            launcher.clear_start_failure(cx);
        });
        let client = daemon.read(cx).client();
        Self {
            daemon,
            client,
            executable,
            state: LaunchState::Idle,
            process: None,
            unopened: None,
            restart: false,
            start_failed: false,
            stopped_by_user: false,
            stuck: false,
            _task: None,
            _online: online,
        }
    }

    pub fn state(&self) -> &LaunchState {
        &self.state
    }

    /// What the app is doing to the daemon, for the headline.
    pub fn activity(&self) -> Launch {
        match &self.state {
            LaunchState::Starting => Launch::Starting,
            LaunchState::Stopping if self.restart => Launch::Restarting,
            LaunchState::Stopping => Launch::Stopping,
            LaunchState::Failed(reason) => Launch::Failed(reason.to_string()),
            LaunchState::Idle if self.stopped_by_user => Launch::StoppedByUser,
            LaunchState::Idle => Launch::Idle,
        }
    }

    /// Whether a daemon is running without answering, so ending it is the
    /// way out.
    pub fn is_stuck(&self) -> bool {
        self.stuck
    }

    /// The daemon's log, when this app started it and the file exists.
    pub fn log_file(&self) -> Option<PathBuf> {
        self.executable
            .as_ref()
            .map(|executable| work_dir(executable).join(LOG_FILE))
            .filter(|log| log.is_file())
    }

    /// Ends the daemon the app uses, which isn't answering, then starts a new
    /// one if `restart`. Only that process: any other `vrft_d.exe` is left
    /// alone.
    pub fn end_stuck(&mut self, restart: bool, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        let Some(process) = self.running_process() else {
            if self.process.is_some() {
                // It has just stopped by itself.
                self.ended(restart, cx);
            } else {
                self.fail(t!("launcher.not_answering"), cx);
            }
            return;
        };
        self.state = LaunchState::Stopping;
        cx.notify();
        self._task = Some(cx.spawn(async move |this, cx| {
            let ended = cx
                .background_executor()
                .spawn(async move {
                    process.end()?;
                    if process.wait(END_TIMEOUT) {
                        Ok(())
                    } else {
                        Err(std::io::Error::other(format!(
                            "{DAEMON_EXE} is still running"
                        )))
                    }
                })
                .await;
            this.update(cx, |launcher, cx| match ended {
                Ok(()) => launcher.ended(restart, cx),
                Err(error) => launcher.fail(t!("launcher.couldnt_end", error = error), cx),
            })
            .ok();
        }));
    }

    /// The daemon that wasn't answering is gone; starts another if `restart`.
    fn ended(&mut self, restart: bool, cx: &mut Context<Self>) {
        self.process = None;
        self.stopped_by_user = !restart;
        self.finish(cx);
        if restart {
            self.start(cx);
        }
    }

    /// Reads as stopped by the user without starting anything, for
    /// capturing Home in development.
    pub fn hold_stopped(&mut self) {
        self.stopped_by_user = true;
    }

    pub fn is_busy(&self) -> bool {
        matches!(self.state, LaunchState::Starting | LaunchState::Stopping)
    }

    /// Starts the daemon unless one is already running, such as one started
    /// from a console, which the app then simply connects to.
    pub fn start_unless_running(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let running = cx
                .background_executor()
                .spawn(async move { daemon_running(&client) })
                .await;
            if !running {
                this.update(cx, |launcher, cx| launcher.start(cx)).ok();
            }
        })
        .detach();
    }

    pub fn start(&mut self, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        let Some(executable) = self.executable.clone() else {
            self.fail_start(t!("launcher.no_executable"), cx);
            return;
        };
        self.state = LaunchState::Starting;
        self.stopped_by_user = false;
        cx.notify();
        let client = self.client.clone();
        self._task = Some(cx.spawn(async move |this, cx| {
            let log = work_dir(&executable).join(LOG_FILE);
            let spawn_log = log.clone();
            // Starting a process can pump the window's message loop, so it
            // happens off the UI thread.
            let started = cx
                .background_executor()
                .spawn(async move {
                    if daemon_running(&client) {
                        return Ok(None);
                    }
                    spawn_daemon(&executable, &spawn_log).map(Some)
                })
                .await;
            let child: Option<Child> = match started {
                Ok(child) => child,
                Err(error) => {
                    this.update(cx, |launcher, cx| {
                        launcher.fail_start(t!("launcher.couldnt_start", error = error), cx)
                    })
                    .ok();
                    return;
                }
            };
            let spawned = child.is_some();
            if let Some(child) = child {
                this.update(cx, |launcher, _| {
                    launcher.process = Some(DaemonProcess {
                        process: Arc::new(Process::from_child(child)),
                        started_here: true,
                    });
                })
                .ok();
            }
            let began = Instant::now();
            loop {
                cx.background_executor().timer(CHECK_INTERVAL).await;
                let Ok(online) = this.update(cx, |launcher, cx| launcher.online(cx)) else {
                    return;
                };
                if online {
                    this.update(cx, |launcher, cx| launcher.finish(cx)).ok();
                    return;
                }
                let exited = this
                    .update(cx, |launcher, _| launcher.started_exit())
                    .ok()
                    .flatten();
                if let Some(status) = exited {
                    let reason = fs::read_to_string(&log)
                        .ok()
                        .and_then(|text| reason_from_log(&text))
                        .unwrap_or_else(|| t!("launcher.exited_with", status = status).into());
                    this.update(cx, |launcher, cx| {
                        launcher
                            .fail_start(t!("launcher.stopped_straight_away", reason = reason), cx)
                    })
                    .ok();
                    return;
                }
                if began.elapsed() > START_TIMEOUT {
                    let message = if spawned {
                        t!("launcher.not_answering_log", log = log.display())
                    } else {
                        t!("launcher.not_answering")
                    };
                    this.update(cx, |launcher, cx| {
                        launcher.fail_start(message, cx);
                        launcher.stuck = launcher.running_process().is_some();
                    })
                    .ok();
                    return;
                }
            }
        }));
    }

    /// Stops the daemon and starts it again, such as for a change to its
    /// config to take effect. Starts it if it isn't running.
    pub fn restart(&mut self, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        if self.offline(cx) {
            self.start(cx);
        } else {
            self.restart = true;
            self.stop(cx);
        }
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        self.state = LaunchState::Stopping;
        cx.notify();
        let client = self.client.clone();
        let process = self.running_process();
        self._task = Some(cx.spawn(async move |this, cx| {
            let asked = cx
                .background_executor()
                .spawn(async move { client.shutdown() })
                .await;
            if let Err(error) = asked {
                this.update(cx, |launcher, cx| {
                    launcher.fail(error.to_string(), cx);
                    // Too busy to answer, perhaps: ending it is the way out.
                    launcher.stuck = launcher.running_process().is_some();
                })
                .ok();
                return;
            }
            let began = Instant::now();
            let mut exited_at = None;
            loop {
                cx.background_executor().timer(CHECK_INTERVAL).await;
                let Ok(offline) = this.update(cx, |launcher, cx| launcher.offline(cx)) else {
                    return;
                };
                // The process itself must be gone: starting again while it
                // still exits would find it running and start nothing. A
                // daemon too old to say which process it is, is gone once
                // it stops answering.
                let exited = match &process {
                    Some(process) => process.has_exited(),
                    None => offline,
                };
                // Stopping lasts until the app sees it gone too, so what's
                // shown doesn't flash back to its last status in between.
                let seen_gone = offline
                    || (exited
                        && exited_at.get_or_insert_with(Instant::now).elapsed() > GONE_GRACE);
                if exited && seen_gone {
                    this.update(cx, |launcher, cx| {
                        launcher.forget_exited();
                        let restart = std::mem::take(&mut launcher.restart);
                        launcher.stopped_by_user = !restart;
                        launcher.finish(cx);
                        if restart {
                            launcher.start(cx);
                        }
                    })
                    .ok();
                    return;
                }
                if began.elapsed() > STOP_TIMEOUT {
                    this.update(cx, |launcher, cx| {
                        launcher.fail(t!("launcher.didnt_stop"), cx);
                        launcher.stuck = launcher.running_process().is_some();
                    })
                    .ok();
                    return;
                }
            }
        }));
    }

    fn online(&self, cx: &App) -> bool {
        *self.daemon.read(cx).connection() == Connection::Online
    }

    fn offline(&self, cx: &App) -> bool {
        matches!(
            self.daemon.read(cx).connection(),
            Connection::Offline { .. }
        )
    }

    /// Drops a start failure once the daemon answers after all.
    fn clear_start_failure(&mut self, cx: &mut Context<Self>) {
        if self.start_failed && matches!(self.state, LaunchState::Failed(_)) && self.online(cx) {
            self.start_failed = false;
            self.finish(cx);
        }
    }

    fn finish(&mut self, cx: &mut Context<Self>) {
        self.stuck = false;
        self.state = LaunchState::Idle;
        cx.notify();
    }

    fn fail_start(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.fail(message, cx);
        self.start_failed = true;
    }

    fn fail(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.restart = false;
        self.start_failed = false;
        self.stuck = false;
        self.state = LaunchState::Failed(message.into());
        cx.notify();
    }

    /// Holds the daemon that answers, by the process id it reports, so
    /// stopping or ending it reaches that process and no other.
    fn follow_answering_daemon(&mut self, cx: &App) {
        let Some(pid) = self
            .daemon
            .read(cx)
            .status()
            .and_then(|status| status.daemon.as_ref()?.pid)
        else {
            return;
        };
        let held = self
            .process
            .as_ref()
            .is_some_and(|daemon| daemon.process.pid() == pid && !daemon.process.has_exited());
        if held || self.unopened == Some(pid) {
            return;
        }
        match open_daemon(pid) {
            Some(process) => {
                self.process = Some(DaemonProcess {
                    process: Arc::new(process),
                    started_here: false,
                });
                self.unopened = None;
            }
            None => {
                log::warn!("Couldn't open vrft_d (process {pid}), so this app can't end it");
                self.unopened = Some(pid);
            }
        }
    }

    /// The daemon process the app uses, while it runs.
    fn running_process(&self) -> Option<Arc<Process>> {
        self.process
            .as_ref()
            .filter(|daemon| !daemon.process.has_exited())
            .map(|daemon| daemon.process.clone())
    }

    fn forget_exited(&mut self) {
        if self.running_process().is_none() {
            self.process = None;
        }
    }

    /// How the daemon this app started exited, if it has. It's then no longer
    /// this app's to stop.
    fn started_exit(&mut self) -> Option<ExitStatus> {
        let daemon = self.process.as_ref().filter(|daemon| daemon.started_here)?;
        let status = daemon.process.exit_status()?;
        self.process = None;
        Some(status)
    }

    /// Stops the daemon this app started, as the app closes. It's asked to
    /// shut down first, and only ended if it hasn't stopped within
    /// `QUIT_TIMEOUT`. Were the app to crash instead, the daemon stops by
    /// itself as the app goes.
    fn stop_started(&mut self) {
        let Some(daemon) = self.process.take().filter(|daemon| daemon.started_here) else {
            return;
        };
        if daemon.process.has_exited() {
            return;
        }
        if let Err(error) = self.client.shutdown() {
            log::warn!("Couldn't ask vrft_d to stop: {error:#}");
        }
        if daemon.process.wait(QUIT_TIMEOUT) {
            return;
        }
        log::warn!("vrft_d didn't stop within {QUIT_TIMEOUT:?}, so it's being ended");
        if let Err(error) = daemon.process.end() {
            log::warn!("Couldn't end vrft_d: {error}");
        }
        daemon.process.wait(END_TIMEOUT);
    }
}

/// Closing the window releases this before GPUI runs its quit handlers, so
/// an `on_app_quit` handler here would never run. Dropping covers every way
/// the app closes short of it being killed.
impl Drop for Launcher {
    fn drop(&mut self) {
        self.stop_started();
    }
}

/// Process `pid`, if it's a `vrft_d.exe` this app can end.
fn open_daemon(pid: u32) -> Option<Process> {
    let process = Process::open(pid)?;
    let name = process.executable()?;
    name.file_name()?
        .to_str()?
        .eq_ignore_ascii_case(DAEMON_EXE)
        .then_some(process)
}

/// The folder the daemon runs in, which holds `config.json` and `plugins/`:
/// an installed copy's data folder, or else the daemon's own folder. A
/// development build there moves itself to the repository root.
fn work_dir(executable: &Path) -> PathBuf {
    let own = executable.parent().unwrap_or(Path::new("."));
    vrft_protocol::layout::installed_data_dir(own).unwrap_or_else(|| own.to_path_buf())
}

fn spawn_daemon(executable: &Path, log: &Path) -> std::io::Result<Child> {
    let command = |flags: u32| -> std::io::Result<Command> {
        let dir = work_dir(executable);
        // An installed copy's daemon fills the data folder as it starts, but
        // its log goes there first.
        fs::create_dir_all(&dir)?;
        let output = fs::File::create(log)?;
        let mut command = Command::new(executable);
        command
            .arg(OWNER_PID_ARG)
            .arg(std::process::id().to_string())
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(output.try_clone()?)
            .stderr(output);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            command.creation_flags(flags);
        }
        #[cfg(not(windows))]
        let _ = flags;
        Ok(command)
    };
    // No console window, and no Ctrl-C from a console the app was started
    // from. Leaving the app's job, where Windows allows it, keeps the daemon
    // alive when whatever started the app closes.
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let flags = CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP;
    match command(flags | CREATE_BREAKAWAY_FROM_JOB)?.spawn() {
        Err(error) if error.raw_os_error() == Some(5) => command(flags)?.spawn(),
        result => result,
    }
}

/// Whether a daemon is already running, perhaps still starting up: one
/// holds the daemon's mutex, or one answers, if it's too old to hold it.
/// Another `vrft_d.exe`, such as a tongue training run, isn't a daemon.
fn daemon_running(client: &DaemonClient) -> bool {
    crate::processes::named_mutex_exists(DAEMON_INSTANCE) || client.status().is_ok()
}

/// The last error the daemon logged, or else its last line, without the
/// logger's timestamp and level.
fn reason_from_log(text: &str) -> Option<String> {
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let line = lines
        .iter()
        .rev()
        .find(|line| line.contains(" ERROR "))
        .or(lines.last())?;
    let message = match line.strip_prefix('[') {
        Some(rest) => rest.split_once("] ").map_or(*line, |(_, message)| message),
        None => line,
    };
    Some(message.trim().to_string())
}

/// The offer to start VRFT: a button, and why starting failed if it did.
#[derive(IntoElement)]
pub struct StartVrft {
    launcher: Entity<Launcher>,
    prominent: bool,
    buttons_only: bool,
}

impl StartVrft {
    pub fn new(launcher: Entity<Launcher>) -> Self {
        Self {
            launcher,
            prominent: false,
            buttons_only: false,
        }
    }

    /// Full-size buttons, for the one action on a page.
    pub fn prominent(mut self) -> Self {
        self.prominent = true;
        self
    }

    /// Just the buttons, for where Stop sits while tracking runs. The page
    /// says why starting failed.
    pub fn buttons_only(mut self) -> Self {
        self.buttons_only = true;
        self
    }
}

impl RenderOnce for StartVrft {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = self.launcher.read(cx).state().clone();
        let starting = state == LaunchState::Starting;
        let stuck = self.launcher.read(cx).is_stuck();
        let log = self.launcher.read(cx).log_file();
        let launcher = self.launcher.clone();
        let end_launcher = self.launcher.clone();
        let prominent = self.prominent;
        let size = move |button: Button| {
            if prominent {
                button.h_10().px_4()
            } else {
                button.small().h_8().px_3()
            }
        };
        let start = size(
            Button::new("start-vrft")
                .primary()
                .icon(IconName::Play)
                .label(if starting {
                    t!("launcher.starting")
                } else {
                    t!("launcher.start")
                })
                .loading(starting)
                .disabled(starting || state == LaunchState::Stopping)
                .on_click(move |_, _, cx| {
                    launcher.update(cx, |launcher, cx| launcher.start(cx));
                }),
        );
        let buttons = h_flex()
            .gap_2()
            .child(start)
            .when_some(log, |row, log| {
                row.child(size(
                    Button::new("open-log")
                        .ghost()
                        .icon(IconName::FileText)
                        .label(t!("launcher.open_log"))
                        .on_click(move |_, _, cx| cx.open_with_system(&log)),
                ))
            })
            .when(stuck, |row| {
                row.child(size(
                    Button::new("end-vrft")
                        .danger()
                        .outline()
                        .label(t!("launcher.end_and_start"))
                        .on_click(move |_, _, cx| {
                            end_launcher.update(cx, |launcher, cx| launcher.end_stuck(true, cx))
                        }),
                ))
            });
        let failure = match state {
            LaunchState::Failed(message) if !self.buttons_only => {
                Some(Notice::new(Tone::Problem, message.clone()).details(message))
            }
            _ => None,
        };
        v_flex()
            .gap_3()
            .items_start()
            .children(failure)
            .child(buttons)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_reason_prefers_the_last_error() {
        let log = "[2026-09-25T14:47:47Z INFO  vrft_d] Starting...\n\
                   [2026-09-25T14:47:47Z ERROR vrft_d] Failed to initialize transport manager: address in use\n\
                   [2026-09-25T14:47:48Z INFO  vrft_d] Shutting down...\n";
        assert_eq!(
            reason_from_log(log).as_deref(),
            Some("Failed to initialize transport manager: address in use")
        );
    }

    #[test]
    fn log_reason_falls_back_to_the_last_line() {
        assert_eq!(
            reason_from_log("[2026-09-25T14:47:47Z INFO  vrft_d] Starting...\n\n").as_deref(),
            Some("Starting...")
        );
        assert_eq!(
            reason_from_log("thread 'main' panicked").as_deref(),
            Some("thread 'main' panicked")
        );
        assert_eq!(reason_from_log(""), None);
    }
}
