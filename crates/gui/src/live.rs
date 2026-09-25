//! Live state polled from the daemon: its status, and the mouth camera image
//! while a page shows it.
use crate::daemon::{Camera, DaemonClient, Frame, Settings, Status, FRAME_HEIGHT, FRAME_WIDTH};
use crate::summary::{Connection, RateMeter, Rates};
use gpui_kit::{Context, RenderImage, Task};
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
    camera: RateMeter,
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
            camera: RateMeter::default(),
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
                let now = Instant::now();
                if let Some(daemon) = &status.daemon {
                    self.tracking.record(now, daemon.tracking_frames);
                }
                match status.sequence {
                    Some(sequence) => self.camera.record(now, sequence),
                    None => self.camera.clear(),
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
                self.camera.clear();
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

    /// Shows settings the daemon just confirmed without waiting for the next
    /// status.
    pub fn show_settings(&mut self, settings: Settings, cx: &mut Context<Self>) {
        if let Some(status) = &mut self.status {
            status.settings = settings;
            cx.notify();
        }
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
            camera_fps: self.camera.per_second(),
        }
    }
}

const IDLE_INTERVAL: Duration = Duration::from_millis(250);

/// The latest image from one camera pair. It only downloads frames while a
/// page is watching, and releases the image's GPU memory when none is.
pub struct CameraFeed {
    image: Option<Arc<RenderImage>>,
    /// The image shown before `image`. Released one frame later, so the GPU
    /// never loses a texture that a frame on screen still uses.
    previous: Option<Arc<RenderImage>>,
    sequence: Option<u64>,
    watching: bool,
    _poll: Task<()>,
}

impl CameraFeed {
    pub fn new(client: Arc<DaemonClient>, camera: Camera, cx: &mut Context<Self>) -> Self {
        // The mouth stream runs at the headset's camera rate, 24 FPS by
        // default, so check a little faster than that. Eye snapshots arrive a
        // few times a second at most.
        let interval = match camera {
            Camera::Mouth => Duration::from_millis(30),
            Camera::Eyes => Duration::from_millis(200),
        };
        let poll = cx.spawn(async move |this, cx| loop {
            let Ok((watching, shown)) =
                this.update(cx, |camera, _| (camera.watching, camera.sequence))
            else {
                break;
            };
            if !watching {
                cx.background_executor().timer(IDLE_INTERVAL).await;
                continue;
            }
            let client = client.clone();
            let frame = cx
                .background_executor()
                .spawn(async move { client.frame(camera, shown).ok().flatten().map(decode) })
                .await;
            if let Some((sequence, image)) = frame {
                if this
                    .update(cx, |camera, cx| camera.show(sequence, image, cx))
                    .is_err()
                {
                    break;
                }
            }
            cx.background_executor().timer(interval).await;
        });
        Self {
            image: None,
            previous: None,
            sequence: None,
            watching: false,
            _poll: poll,
        }
    }

    fn show(&mut self, sequence: u64, image: Arc<RenderImage>, cx: &mut Context<Self>) {
        if !self.watching || self.sequence == Some(sequence) {
            return;
        }
        if let Some(stale) = self.previous.take() {
            cx.drop_image(stale, None);
        }
        self.previous = self.image.replace(image);
        self.sequence = Some(sequence);
        cx.notify();
    }

    pub fn set_watching(&mut self, watching: bool, cx: &mut Context<Self>) {
        if self.watching == watching {
            return;
        }
        self.watching = watching;
        if !watching {
            for image in [self.image.take(), self.previous.take()]
                .into_iter()
                .flatten()
            {
                cx.drop_image(image, None);
            }
            self.sequence = None;
        }
        cx.notify();
    }

    pub fn image(&self) -> Option<Arc<RenderImage>> {
        self.image.clone()
    }
}

pub fn decode(frame: Frame) -> (u64, Arc<RenderImage>) {
    let buffer = image::RgbaImage::from_raw(FRAME_WIDTH, FRAME_HEIGHT, frame.to_bgra())
        .expect("a checked frame fills the image");
    let image = RenderImage::new(vec![image::Frame::new(buffer)]);
    (frame.sequence, Arc::new(image))
}
