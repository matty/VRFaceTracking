//! Keeps an installed copy of the app up to date from the repository's
//! GitHub releases, with Velopack.
//!
//! A release build follows the `stable` channel, the CalVer releases. A dev
//! build, whose version carries `-dev`, follows the `dev` channel, the
//! rolling `dev` prerelease that every push to main republishes. The app
//! checks when it opens and every few hours, downloads what it finds, and
//! installs it when restarted: from the button here, or by itself the next
//! time it opens. A copy that wasn't installed, such as one unpacked from a
//! zip or a development build, can't update itself.
use gpui_kit::{Context, SharedString, Task, Window};
use rust_i18n::t;
use std::time::Duration;
use velopack::sources::GithubSource;
use velopack::{UpdateCheck, UpdateInfo, UpdateManager, UpdateOptions, VelopackAsset};

/// This build's version, `YYYY.M.N`, or `YYYY.M.N-dev.C` for a dev build.
pub const VERSION: &str = env!("VRFT_VERSION");
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
/// How long after opening the app first checks, so it doesn't hold up
/// starting tracking.
const FIRST_CHECK: Duration = Duration::from_secs(10);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// Whether this is a dev build, which follows the `dev` channel and says so
/// in its name.
pub fn is_dev() -> bool {
    VERSION.contains("-dev")
}

/// The app's name, with "(Dev)" for a dev build.
pub fn app_name() -> SharedString {
    if is_dev() {
        t!("app.title_dev").into()
    } else {
        t!("app.title").into()
    }
}

/// The release channel this build follows, as the release workflow packs it.
fn channel() -> &'static str {
    if is_dev() {
        "dev"
    } else {
        "stable"
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum UpdateState {
    /// Not an installed copy, so it can't update itself.
    Unavailable,
    /// Not checked yet.
    Idle,
    Checking,
    UpToDate,
    Downloading(String),
    /// Downloaded, and installed when the app restarts.
    Ready(String),
    Failed(String),
}

pub struct Updater {
    manager: Option<UpdateManager>,
    state: UpdateState,
    /// The downloaded update, while the state is `Ready`.
    ready: Option<VelopackAsset>,
    _check: Option<Task<()>>,
    _schedule: Option<Task<()>>,
}

impl Updater {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let source = GithubSource::new(REPOSITORY, None, is_dev());
        let options = UpdateOptions {
            ExplicitChannel: Some(channel().into()),
            ..Default::default()
        };
        // Fails for a copy that wasn't installed.
        let manager = UpdateManager::new(source, Some(options), None)
            .inspect_err(|error| log::info!("Updates are off: {error}"))
            .ok();
        let ready = manager
            .as_ref()
            .and_then(UpdateManager::get_update_pending_restart);
        let state = match (&manager, &ready) {
            (None, _) => UpdateState::Unavailable,
            (Some(_), Some(asset)) => UpdateState::Ready(asset.Version.clone()),
            (Some(_), None) => UpdateState::Idle,
        };
        let schedule = manager.is_some().then(|| {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(FIRST_CHECK).await;
                loop {
                    if this.update(cx, |updater, cx| updater.check(cx)).is_err() {
                        break;
                    }
                    cx.background_executor().timer(CHECK_EVERY).await;
                }
            })
        });
        Self {
            manager,
            state,
            ready,
            _check: None,
            _schedule: schedule,
        }
    }

    pub fn state(&self) -> &UpdateState {
        &self.state
    }

    fn busy(&self) -> bool {
        matches!(
            self.state,
            UpdateState::Checking | UpdateState::Downloading(_)
        )
    }

    /// Looks for a newer version and downloads it. Does nothing while busy
    /// or once an update is waiting to install.
    pub fn check(&mut self, cx: &mut Context<Self>) {
        let Some(manager) = self.manager.clone() else {
            return;
        };
        if self.busy() || self.ready.is_some() {
            return;
        }
        self.state = UpdateState::Checking;
        cx.notify();
        self._check = Some(cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn({
                    let manager = manager.clone();
                    async move { manager.check_for_updates() }
                })
                .await;
            let info: Box<UpdateInfo> = match found {
                Ok(UpdateCheck::UpdateAvailable(info)) => info,
                Ok(UpdateCheck::NoUpdateAvailable | UpdateCheck::RemoteIsEmpty) => {
                    let _ = this.update(cx, |updater, cx| {
                        updater.state = UpdateState::UpToDate;
                        cx.notify();
                    });
                    return;
                }
                Err(error) => {
                    log::warn!("Couldn't check for updates: {error}");
                    let _ = this.update(cx, |updater, cx| {
                        updater.state = UpdateState::Failed(error.to_string());
                        cx.notify();
                    });
                    return;
                }
            };
            let version = info.TargetFullRelease.Version.clone();
            log::info!("Downloading update {version}");
            if this
                .update(cx, |updater, cx| {
                    updater.state = UpdateState::Downloading(version.clone());
                    cx.notify();
                })
                .is_err()
            {
                return;
            }
            let downloaded = cx
                .background_executor()
                .spawn({
                    let info = info.clone();
                    async move { manager.download_updates(&info, None) }
                })
                .await;
            let _ = this.update(cx, |updater, cx| {
                match downloaded {
                    Ok(()) => {
                        updater.state = UpdateState::Ready(version);
                        updater.ready = Some(info.TargetFullRelease);
                    }
                    Err(error) => {
                        log::warn!("Couldn't download update {version}: {error}");
                        updater.state = UpdateState::Failed(error.to_string());
                    }
                }
                cx.notify();
            });
        }));
    }

    /// Hands the downloaded update to the installer and closes the app,
    /// which stops tracking it started. The installer then reopens it.
    pub fn restart_to_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(manager), Some(asset)) = (&self.manager, &self.ready) else {
            return;
        };
        match manager.wait_exit_then_apply_updates(asset, false, true, Vec::<String>::new()) {
            // Closing the last window quits the app, as closing it by hand
            // does, so the daemon it started stops first.
            Ok(()) => window.remove_window(),
            Err(error) => {
                log::warn!("Couldn't start installing the update: {error}");
                self.state = UpdateState::Failed(error.to_string());
                cx.notify();
            }
        }
    }
}
