//! Live state polled from the daemon: its status, including each running
//! extension's.
use crate::client::{DaemonClient, ExtensionReport, Status};
use crate::summary::{Connection, RateMeter, Rates};
use gpui_kit::{Context, Task};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Status polling is quick while a page animates live values, and relaxed
/// otherwise.
const STATUS_INTERVAL_FAST: Duration = Duration::from_millis(100);
const STATUS_INTERVAL: Duration = Duration::from_millis(400);
/// A single failed poll can be a daemon busy starting up; two in a row mean
/// it has gone.
const FAILURES_BEFORE_OFFLINE: u32 = 2;

pub struct DaemonState {
    client: Arc<DaemonClient>,
    connection: Connection,
    status: Option<Status>,
    failures: u32,
    tracking: RateMeter,
    fast: bool,
    _poll: Task<()>,
}

impl DaemonState {
    pub fn new(client: Arc<DaemonClient>, cx: &mut Context<Self>) -> Self {
        let poll_client = client.clone();
        let poll = cx.spawn(async move |this, cx| loop {
            let client = poll_client.clone();
            let result = cx
                .background_executor()
                .spawn(async move { client.status() })
                .await;
            let Ok(interval) = this.update(cx, |state, cx| {
                state.apply(result, cx);
                state.interval()
            }) else {
                break;
            };
            cx.background_executor().timer(interval).await;
        });
        Self {
            client,
            connection: Connection::Connecting,
            status: None,
            failures: 0,
            tracking: RateMeter::default(),
            fast: false,
            _poll: poll,
        }
    }

    fn interval(&self) -> Duration {
        if self.fast && self.connection == Connection::Online {
            STATUS_INTERVAL_FAST
        } else {
            STATUS_INTERVAL
        }
    }

    fn apply(&mut self, result: anyhow::Result<Status>, cx: &mut Context<Self>) {
        match result {
            Ok(status) => {
                if let Some(daemon) = &status.daemon {
                    self.tracking.record(Instant::now(), daemon.tracking_frames);
                }
                self.failures = 0;
                self.connection = Connection::Online;
                self.status = Some(status);
            }
            Err(error) => {
                self.failures += 1;
                if self.failures < FAILURES_BEFORE_OFFLINE {
                    return;
                }
                self.connection = Connection::Offline {
                    error: format!("{error:#}"),
                };
                self.status = None;
                self.tracking.clear();
            }
        }
        cx.notify();
    }

    /// Poll faster while a page shows values that move with the wearer.
    pub fn set_fast(&mut self, fast: bool) {
        self.fast = fast;
    }

    pub fn address(&self) -> &str {
        self.client.address()
    }

    pub fn client(&self) -> Arc<DaemonClient> {
        self.client.clone()
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// The latest status while the daemon is reachable.
    pub fn status(&self) -> Option<&Status> {
        self.status.as_ref()
    }

    pub fn rates(&self) -> Rates {
        Rates {
            tracking_fps: self.tracking.per_second(),
        }
    }

    /// What the daemon says about extension `id`, while it's reachable and
    /// new enough to say.
    pub fn extension(&self, id: &str) -> Option<&ExtensionReport> {
        self.status.as_ref()?.daemon.as_ref()?.extension(id)
    }

    /// Extension `id`'s own status, while it runs.
    pub fn extension_status(&self, id: &str) -> Option<&serde_json::Value> {
        self.status.as_ref()?.extensions.get(id)
    }
}
