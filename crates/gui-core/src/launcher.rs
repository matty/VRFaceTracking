//! Starts and stops the daemon, `vrft_d`, for the app.
//!
//! The app needs the daemon for everything it shows, so it starts the daemon
//! when it opens, and offers to start it again whenever it isn't running. The
//! daemon runs without a console window, with its output in `vrft_d.log` in
//! the folder it runs in. A daemon the app started stops when the app closes; one that was already
//! running, such as one started from a console, is left alone.
use crate::client::DaemonClient;
use crate::live::DaemonState;
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

const DAEMON_EXE: &str = "vrft_d.exe";
const LOG_FILE: &str = "vrft_d.log";
/// Loading a .NET module or the tongue runtime can take a while, but not this
/// long.
const START_TIMEOUT: Duration = Duration::from_secs(30);
/// The daemon stops within a second or two once asked.
const STOP_TIMEOUT: Duration = Duration::from_secs(15);
const CHECK_INTERVAL: Duration = Duration::from_millis(250);
/// How long closing the app waits for its daemon to stop by itself before
/// ending it. The daemon puts the headset's eye model back as it stops.
const QUIT_TIMEOUT: Duration = Duration::from_secs(5);

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
    /// The daemon this app started, which it stops when it closes.
    managed: Option<Child>,
    /// Start the daemon again once it has stopped.
    restart: bool,
    /// The failure shown is from starting, so it clears once the daemon
    /// answers after all, such as a slow module that outlasted the timeout.
    start_failed: bool,
    /// The user stopped the daemon, and it hasn't been started since.
    stopped_by_user: bool,
    /// A daemon is running but not answering, so the app offers to end it.
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
        let online = cx.observe(&daemon, |launcher, _, cx| launcher.clear_start_failure(cx));
        let client = daemon.read(cx).client();
        Self {
            daemon,
            client,
            executable,
            state: LaunchState::Idle,
            managed: None,
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

    /// Ends a daemon that isn't answering, then starts a new one if `restart`.
    pub fn end_stuck(&mut self, restart: bool, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        self.state = LaunchState::Stopping;
        cx.notify();
        self._task = Some(cx.spawn(async move |this, cx| {
            let ended = cx
                .background_executor()
                .spawn(async { crate::processes::end(DAEMON_EXE) })
                .await;
            this.update(cx, |launcher, cx| match ended {
                Ok(()) => {
                    launcher.stuck = false;
                    launcher.managed = None;
                    launcher.stopped_by_user = !restart;
                    launcher.finish(cx);
                    if restart {
                        launcher.start(cx);
                    }
                }
                Err(error) => launcher.fail(t!("launcher.couldnt_end", error = error), cx),
            })
            .ok();
        }));
    }

    /// Reads as stopped by the user without starting anything, for
    /// capturing Home in development.
    pub fn hold_stopped(&mut self) {
        self.stopped_by_user = true;
    }

    pub fn is_busy(&self) -> bool {
        matches!(self.state, LaunchState::Starting | LaunchState::Stopping)
    }

    /// Starts the daemon unless a `vrft_d.exe` is already running, such as one
    /// started from a console, which the app then simply connects to.
    pub fn start_unless_running(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let running = cx
                .background_executor()
                .spawn(async { daemon_running() })
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
        self._task = Some(cx.spawn(async move |this, cx| {
            let log = work_dir(&executable).join(LOG_FILE);
            let spawn_log = log.clone();
            // Starting a process can pump the window's message loop, so it
            // happens off the UI thread.
            let started = cx
                .background_executor()
                .spawn(async move {
                    if daemon_running() {
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
                this.update(cx, |launcher, _| launcher.managed = Some(child))
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
                    .update(cx, |launcher, _| launcher.managed_exit())
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
                        launcher.stuck = true;
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
        let client = self.daemon.read(cx).client();
        self._task = Some(cx.spawn(async move |this, cx| {
            let asked = cx
                .background_executor()
                .spawn(async move { client.shutdown() })
                .await;
            if let Err(error) = asked {
                this.update(cx, |launcher, cx| launcher.fail(error.to_string(), cx))
                    .ok();
                return;
            }
            let began = Instant::now();
            loop {
                cx.background_executor().timer(CHECK_INTERVAL).await;
                let Ok(offline) = this.update(cx, |launcher, cx| launcher.offline(cx)) else {
                    return;
                };
                // It stops answering before it exits, such as while it puts
                // the headset's eye model back. Starting again before then
                // would find it still running and start nothing.
                let exited = offline
                    && cx
                        .background_executor()
                        .spawn(async { !daemon_running() })
                        .await;
                if exited {
                    this.update(cx, |launcher, cx| {
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
                        launcher.stuck = true;
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

    /// How the daemon this app started exited, if it has. It's then no longer
    /// this app's to stop.
    fn managed_exit(&mut self) -> Option<ExitStatus> {
        let status = self.managed.as_mut()?.try_wait().ok().flatten()?;
        self.managed = None;
        Some(status)
    }

    /// Stops the daemon this app started, as the app closes. It's asked to
    /// shut down first, so it can put the headset's eye model back, and only
    /// ended if it hasn't stopped within `QUIT_TIMEOUT`.
    fn stop_managed(&mut self) {
        let Some(mut child) = self.managed.take() else {
            return;
        };
        let running = |child: &mut Child| matches!(child.try_wait(), Ok(None));
        if !running(&mut child) {
            return;
        }
        if let Err(error) = self.client.shutdown() {
            log::warn!("Couldn't ask vrft_d to stop: {error:#}");
        }
        let began = Instant::now();
        while began.elapsed() < QUIT_TIMEOUT {
            if !running(&mut child) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        log::warn!("vrft_d didn't stop within {QUIT_TIMEOUT:?}, so it's being ended");
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Closing the window releases this before GPUI runs its quit handlers, so
/// an `on_app_quit` handler here would never run. Dropping covers every way
/// the app closes short of it being killed.
impl Drop for Launcher {
    fn drop(&mut self) {
        self.stop_managed();
    }
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

/// Whether a `vrft_d.exe` is already running, perhaps still starting up.
/// Starting another would send every expression to VRChat twice.
fn daemon_running() -> bool {
    !crate::processes::find(DAEMON_EXE).is_empty()
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
    corner: bool,
}

impl StartVrft {
    pub fn new(launcher: Entity<Launcher>) -> Self {
        Self {
            launcher,
            prominent: false,
            corner: false,
        }
    }

    /// Full-size buttons, for the one action on a page.
    pub fn prominent(mut self) -> Self {
        self.prominent = true;
        self
    }

    /// Just the buttons, Start last, for a card's top-right corner where
    /// Stop sits while tracking runs. The page says why starting failed.
    pub fn corner(mut self) -> Self {
        self.corner = true;
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
        let (first, last) = if self.corner {
            (None, Some(start))
        } else {
            (Some(start), None)
        };
        let buttons = h_flex()
            .gap_2()
            .children(first)
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
            })
            .children(last);
        let failure = match state {
            LaunchState::Failed(message) if !self.corner => {
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
