//! Starts and stops the daemon, `vrft_d`, for the app.
//!
//! The app needs the daemon for everything it shows, so it starts the daemon
//! when it opens, and offers to start it again whenever it isn't running. The daemon runs on its own, without a
//! console window, and keeps tracking if the app closes; its output goes to
//! `vrft_d.log` beside it.
use crate::live::DaemonState;
use crate::summary::{Connection, Tone};
use crate::widgets::Notice;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{v_flex, Disableable as _};
use gpui_kit::{
    App, Context, Entity, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Task,
    Window,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const DAEMON_EXE: &str = "vrft_d.exe";
const LOG_FILE: &str = "vrft_d.log";
/// Loading a .NET module or the tongue runtime can take a while, but not this
/// long.
const START_TIMEOUT: Duration = Duration::from_secs(30);
/// The daemon stops within a second or two once asked.
const STOP_TIMEOUT: Duration = Duration::from_secs(15);
const CHECK_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq)]
pub enum LaunchState {
    Idle,
    Starting,
    Stopping,
    Failed(SharedString),
}

pub struct Launcher {
    daemon: Entity<DaemonState>,
    /// `vrft_d.exe` beside this app, if it is there.
    executable: Option<PathBuf>,
    state: LaunchState,
    _task: Option<Task<()>>,
}

impl Launcher {
    pub fn new(daemon: Entity<DaemonState>) -> Self {
        let executable = std::env::current_exe()
            .ok()
            .map(|app| app.with_file_name(DAEMON_EXE))
            .filter(|path| path.is_file());
        Self {
            daemon,
            executable,
            state: LaunchState::Idle,
            _task: None,
        }
    }

    pub fn state(&self) -> &LaunchState {
        &self.state
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
            self.fail(
                "Can't find vrft_d.exe next to this app. Start it yourself, and this window connects as soon as it's up.",
                cx,
            );
            return;
        };
        self.state = LaunchState::Starting;
        cx.notify();
        self._task = Some(cx.spawn(async move |this, cx| {
            let log = executable.with_file_name(LOG_FILE);
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
            let mut child: Option<Child> = match started {
                Ok(child) => child,
                Err(error) => {
                    this.update(cx, |launcher, cx| {
                        launcher.fail(format!("Couldn't start vrft_d: {error}"), cx)
                    })
                    .ok();
                    return;
                }
            };
            let began = Instant::now();
            loop {
                cx.background_executor().timer(CHECK_INTERVAL).await;
                let Ok(online) = this.update(cx, |launcher, cx| launcher.online(cx)) else {
                    return;
                };
                if online {
                    // Dropping the handle leaves the daemon running.
                    this.update(cx, |launcher, cx| launcher.finish(cx)).ok();
                    return;
                }
                if let Some(status) = child.as_mut().and_then(|child| child.try_wait().ok().flatten()) {
                    let reason = fs::read_to_string(&log)
                        .ok()
                        .and_then(|text| reason_from_log(&text))
                        .unwrap_or_else(|| format!("It exited with {status}."));
                    this.update(cx, |launcher, cx| {
                        launcher.fail(format!("VRFT stopped straight away. {reason}"), cx)
                    })
                    .ok();
                    return;
                }
                if began.elapsed() > START_TIMEOUT {
                    let message = if child.is_some() {
                        format!(
                            "VRFT started but isn't answering. Its log is at {}.",
                            log.display()
                        )
                    } else {
                        "vrft_d is already running but isn't answering. Stop it in Task Manager, then start it again.".into()
                    };
                    this.update(cx, |launcher, cx| launcher.fail(message, cx)).ok();
                    return;
                }
            }
        }));
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
                this.update(cx, |launcher, cx| launcher.fail(format!("{error:#}"), cx))
                    .ok();
                return;
            }
            let began = Instant::now();
            loop {
                cx.background_executor().timer(CHECK_INTERVAL).await;
                let Ok(offline) = this.update(cx, |launcher, cx| launcher.offline(cx)) else {
                    return;
                };
                if offline {
                    this.update(cx, |launcher, cx| launcher.finish(cx)).ok();
                    return;
                }
                if began.elapsed() > STOP_TIMEOUT {
                    this.update(cx, |launcher, cx| {
                        launcher.fail("VRFT didn't stop. Close it in Task Manager.", cx)
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

    fn finish(&mut self, cx: &mut Context<Self>) {
        self.state = LaunchState::Idle;
        cx.notify();
    }

    fn fail(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.state = LaunchState::Failed(message.into());
        cx.notify();
    }
}

/// Starts the daemon from its own folder, which holds `config.json` and
/// `plugins/`. A development build there moves itself to the repository root.
fn spawn_daemon(executable: &Path, log: &Path) -> std::io::Result<Child> {
    let command = |flags: u32| -> std::io::Result<Command> {
        let output = fs::File::create(log)?;
        let mut command = Command::new(executable);
        command
            .current_dir(executable.parent().unwrap_or(Path::new(".")))
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
}

impl StartVrft {
    pub fn new(launcher: Entity<Launcher>) -> Self {
        Self { launcher }
    }
}

impl RenderOnce for StartVrft {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = self.launcher.read(cx).state().clone();
        let starting = state == LaunchState::Starting;
        let launcher = self.launcher.clone();
        v_flex()
            .gap_3()
            .items_start()
            .child(
                Button::new("start-vrft")
                    .primary()
                    .label(if starting {
                        "Starting VRFT"
                    } else {
                        "Start VRFT"
                    })
                    .loading(starting)
                    .disabled(starting || state == LaunchState::Stopping)
                    .on_click(move |_, _, cx| {
                        launcher.update(cx, |launcher, cx| launcher.start(cx));
                    }),
            )
            .children(match state {
                LaunchState::Failed(message) => Some(Notice::new(Tone::Problem, message)),
                _ => None,
            })
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
