//! Live Quest Pro state: this extension's part of the daemon's polled status,
//! and the camera images while a page shows them.
use crate::daemon::{Camera, Frame, PupilMark, QuestProClient, Settings, Status};
use crate::summary::{Connection, Rates};
use gpui_kit::{Context, Entity, RenderImage, Subscription, Task};
use std::sync::Arc;
use std::time::{Duration, Instant};
use vrft_gui_core::live::DaemonState;
use vrft_gui_core::summary::RateMeter;

/// The daemon's Quest Pro status, read from each status the app polls.
pub struct QuestProState {
    client: Arc<QuestProClient>,
    connection: Connection,
    status: Option<Status>,
    tracking_fps: Option<f32>,
    camera: RateMeter,
    _observe: Subscription,
}

impl QuestProState {
    pub fn new(daemon: Entity<DaemonState>, cx: &mut Context<Self>) -> Self {
        let client = Arc::new(QuestProClient::new(daemon.read(cx).client()));
        let observe = cx.observe(&daemon, |state, daemon, cx| {
            state.refresh(&daemon, cx);
            cx.notify();
        });
        let mut state = Self {
            client,
            connection: Connection::Connecting,
            status: None,
            tracking_fps: None,
            camera: RateMeter::default(),
            _observe: observe,
        };
        state.refresh(&daemon, cx);
        state
    }

    fn refresh(&mut self, daemon: &Entity<DaemonState>, cx: &mut Context<Self>) {
        let daemon = daemon.read(cx);
        self.connection = daemon.connection().clone();
        self.tracking_fps = daemon.rates().tracking_fps;
        self.status = daemon.extension_status(crate::ID).map(|value| {
            serde_json::from_value(value.clone()).unwrap_or_else(|error| {
                log::warn!("Quest Pro status this app doesn't understand: {error}");
                Status::default()
            })
        });
        match self.status.as_ref().and_then(|status| status.sequence) {
            Some(sequence) => self.camera.record(Instant::now(), sequence),
            None => self.camera.clear(),
        }
    }

    pub fn client(&self) -> Arc<QuestProClient> {
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

    /// The latest Quest Pro status while the daemon is reachable and runs
    /// Quest Pro support.
    pub fn status(&self) -> Option<&Status> {
        self.status.as_ref()
    }

    pub fn rates(&self) -> Rates {
        Rates {
            tracking_fps: self.tracking_fps,
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
    /// For eye snapshots, the pupils found in `image`.
    pupils: Option<[Option<PupilMark>; 2]>,
    watching: bool,
    _poll: Task<()>,
}

impl CameraFeed {
    pub fn new(client: Arc<QuestProClient>, camera: Camera, cx: &mut Context<Self>) -> Self {
        // The mouth stream runs at the headset's camera rate, 24 FPS by
        // default, so check a little faster than that. Eye snapshots arrive a
        // few times a second at most.
        let interval = match camera {
            Camera::Mouth | Camera::Brow => Duration::from_millis(30),
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
                .spawn(async move {
                    client.frame(camera, shown).ok().flatten().map(|frame| {
                        let pupils = frame.pupils;
                        let (sequence, image) = decode(frame);
                        (sequence, image, pupils)
                    })
                })
                .await;
            if let Some((sequence, image, pupils)) = frame {
                if this
                    .update(cx, |camera, cx| camera.show(sequence, image, pupils, cx))
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
            pupils: None,
            watching: false,
            _poll: poll,
        }
    }

    fn show(
        &mut self,
        sequence: u64,
        image: Arc<RenderImage>,
        pupils: Option<[Option<PupilMark>; 2]>,
        cx: &mut Context<Self>,
    ) {
        if !self.watching || self.sequence == Some(sequence) {
            return;
        }
        if let Some(stale) = self.previous.take() {
            cx.drop_image(stale, None);
        }
        self.previous = self.image.replace(image);
        self.sequence = Some(sequence);
        self.pupils = pupils;
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
            self.pupils = None;
        }
        cx.notify();
    }

    pub fn image(&self) -> Option<Arc<RenderImage>> {
        self.image.clone()
    }

    /// For eye snapshots, the pupils found in the image shown.
    pub fn pupils(&self) -> Option<[Option<PupilMark>; 2]> {
        self.pupils
    }
}

pub fn decode(frame: Frame) -> (u64, Arc<RenderImage>) {
    let buffer = image::RgbaImage::from_raw(frame.width, frame.height, frame.to_bgra())
        .expect("a checked frame fills the image");
    let image = RenderImage::new(vec![image::Frame::new(buffer)]);
    (frame.sequence, Arc::new(image))
}
