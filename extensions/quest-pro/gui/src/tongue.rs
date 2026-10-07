//! Training page: guides the wearer through recording tongue poses,
//! training a personal tongue model on them here on this PC, and testing
//! it, each on its own tab. The daemon does the work through its `/capture` and `/training` API;
//! this drives and shows it, as the preview page's Tongue tab does in a
//! browser.
use crate::daemon::{
    BuiltinStage, BuiltinStatus, CaptureCommand, CaptureMode, CaptureStatus, Coverage,
    FaceSetupReport, Models, QuestProClient, Recording, SavedModel, TrainRequest,
    TrainerArchitecture, TrainingDevice, TrainingProgress, TrainingStage, TrainingStatus,
    TransferKind, TransferStatus, CHEEK_POSES,
};
use crate::live::{CameraFeed, QuestProState};
use crate::speech::Speaker;
use crate::summary::{self, Connection, Tone};
use chrono::{DateTime, Local, TimeZone as _};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    h_flex, v_flex, Disableable as _, Icon, Selectable as _, Sizable as _, StyledExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    black, div, img, px, relative, rgb, AnyElement, App, AppContext as _, Context, Div, Entity,
    FocusHandle, Hsla, InteractiveElement as _, IntoElement, KeyBinding, Keystroke, ObjectFit,
    ParentElement, PathPromptOptions, Render, RenderImage, RenderOnce, SharedString,
    StatefulInteractiveElement as _, Styled, StyledImage as _, Subscription, Task, Window,
};

gpui_kit::actions!(tongue_recording, [PauseRecording, SkipPose, StopRecording]);

/// The key context of a running recording's panel.
const RECORDING_KEYS: &str = "TongueRecording";

/// Keys for a running recording, so it can be driven without finding the
/// mouse while wearing the headset. Not Space: that presses the focused
/// button.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("p", PauseRecording, Some(RECORDING_KEYS)),
        KeyBinding::new("s", SkipPose, Some(RECORDING_KEYS)),
        KeyBinding::new("escape", StopRecording, Some(RECORDING_KEYS)),
    ]);
}
use rust_i18n::t;
use std::borrow::Cow;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use vrft_gui_core::launcher::{Launcher, StartVrft};
use vrft_gui_core::palette::{self, MONO_FONT};
use vrft_gui_core::widgets::{
    cap, card, fix_button, hint, mono, ButtonExt as _, EmptyState, Meter, Notice, PageHeader,
    StatusDot, StatusPill,
};

/// Below this content width the cards stack in one column.
const TWO_COLUMN_WIDTH: f32 = 760.;
/// The side column beside the Record and Train steps' main card.
const SIDE_WIDTH: f32 = 336.;
/// The side column beside the Try step's models.
const TRY_SIDE_WIDTH: f32 = 300.;
const COLUMN_GAP: f32 = 20.;
/// What the window keeps above and below a page: the strip across its top
/// and the page's own padding. A recording fills the rest.
const PAGE_CHROME: f32 = 86.;
const MIN_RECORDING_HEIGHT: f32 = 560.;
/// Past this many poses, a recording's progress is one bar, not segments.
const MAX_SEGMENTS: usize = 40;
/// A running recording changes pose every few seconds, so follow it closely.
const RECORDING_INTERVAL: Duration = Duration::from_millis(200);
const IDLE_INTERVAL: Duration = Duration::from_secs(1);
const DEFAULT_EPOCHS: u32 = 12;
/// Recorded frames fetched together while reviewing.
const FRAMES_AT_ONCE: usize = 4;
/// How long a success notice stays.
const SUCCESS_SHOWN_FOR: Duration = Duration::from_secs(6);
/// Recordings left out of training, kept across restarts, beside the
/// daemon's other files in `.local/`.
const UNTICKED_FILE: &str = ".local/tongue-unticked.json";

/// Text in the app's language, looked up when it's shown.
type Text = fn() -> Cow<'static, str>;

/// The recordings the daemon can guide: its mode, name, and what it's for.
const MODES: [(CaptureMode, Text, Text); 5] = [
    (
        CaptureMode::Core,
        || t!("tongue.mode_core"),
        || t!("tongue.mode_core_about"),
    ),
    (
        CaptureMode::Follow,
        || t!("tongue.mode_follow"),
        || t!("tongue.mode_follow_about"),
    ),
    (
        CaptureMode::Direction,
        || t!("tongue.mode_direction"),
        || t!("tongue.mode_direction_about"),
    ),
    (
        CaptureMode::Negatives,
        || t!("tongue.mode_negatives"),
        || t!("tongue.mode_negatives_about"),
    ),
    (
        CaptureMode::Enrollment,
        || t!("tongue.mode_enrollment"),
        || t!("tongue.mode_enrollment_about"),
    ),
];

fn mode_name(mode: Option<CaptureMode>) -> Cow<'static, str> {
    MODES
        .iter()
        .find(|(id, _, _)| Some(*id) == mode)
        .map_or_else(|| t!("tongue.recording"), |(_, name, _)| name())
}

/// The optional recordings, each with an icon and when it helps.
const EXTRAS: [(CaptureMode, IconName, Text); 4] = [
    (CaptureMode::Follow, IconName::Route, || {
        t!("tongue.extra_follow")
    }),
    (CaptureMode::Direction, IconName::Move, || {
        t!("tongue.extra_direction")
    }),
    (CaptureMode::Negatives, IconName::MessageCircle, || {
        t!("tongue.extra_negatives")
    }),
    (CaptureMode::Enrollment, IconName::ScanFace, || {
        t!("tongue.extra_enrollment")
    }),
];

/// What makes a recording go well, shown before starting one.
const TIPS: [Text; 3] = [
    || t!("tongue.tip_aim"),
    || t!("tongue.tip_cameras"),
    || t!("tongue.tip_skip"),
];

/// The steps the page guides through, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Record,
    Train,
    Try,
}

impl Step {
    const ALL: [Step; 3] = [Step::Record, Step::Train, Step::Try];

    fn tab(self) -> Cow<'static, str> {
        match self {
            Step::Record => t!("tongue.tab_record"),
            Step::Train => t!("tongue.tab_train"),
            Step::Try => t!("tongue.tab_test"),
        }
    }
}

/// A count of usable frames, how many the trainer needs, and what it counts.
type Need = (fn(&Coverage) -> u64, u64, &'static str);

/// What the trainer needs across the ticked recordings, in usable frames,
/// named by where the tongue is.
const NEEDS: [Need; 6] = [
    (|c| c.out, 20, "Out"),
    (|c| c.inside, 20, "In"),
    (|c| c.left, 8, "Left"),
    (|c| c.right, 8, "Right"),
    (|c| c.up, 8, "Up"),
    (|c| c.down, 8, "Down"),
];

/// Each need, with how many usable frames the ticked recordings have of it
/// and how many it takes.
fn coverage(recordings: &[&Recording]) -> Vec<(&'static str, u64, u64)> {
    NEEDS
        .iter()
        .map(|(count, needed, name)| {
            let have = recordings
                .iter()
                .map(|recording| count(&recording.coverage))
                .sum();
            (*name, have, *needed)
        })
        .collect()
}

/// The basic pose that gives more of a need `missing` names.
fn missing_pose(need: &str) -> Option<&'static str> {
    Some(match need {
        "Out" => "Tongue straight out",
        "In" => "Neutral",
        "Left" => "Tongue left",
        "Right" => "Tongue right",
        "Up" => "Tongue up",
        "Down" => "Tongue down",
        _ => return None,
    })
}

/// What to call a need on screen.
fn need_label(need: &str) -> Cow<'static, str> {
    match need {
        "Out" => t!("tongue.need_out"),
        "In" => t!("tongue.need_in"),
        "Left" => t!("tongue.need_left"),
        "Right" => t!("tongue.need_right"),
        "Up" => t!("tongue.need_up"),
        "Down" => t!("tongue.need_down"),
        need => Cow::Owned(need.to_string()),
    }
}

/// Puffed frames of each cheek the trainer needs to learn it. Training goes
/// ahead without them, leaving the cheeks to the headset.
const CHEEK_FRAMES: u64 = 8;

/// Whether the ticked recordings have enough of each cheek puffed.
fn cheeks_covered(recordings: &[&Recording]) -> bool {
    let total = |count: fn(&Coverage) -> u64| -> u64 {
        recordings
            .iter()
            .map(|recording| count(&recording.coverage))
            .sum()
    };
    total(|c| c.cheek_left) >= CHEEK_FRAMES && total(|c| c.cheek_right) >= CHEEK_FRAMES
}

/// What the ticked recordings don't have enough of to train.
fn missing(recordings: &[&Recording]) -> Vec<&'static str> {
    coverage(recordings)
        .into_iter()
        .filter(|(_, have, needed)| have < needed)
        .map(|(name, _, _)| name)
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Device {
    Automatic,
    Cpu,
    Gpu,
}

impl Device {
    const ALL: [Device; 3] = [Device::Automatic, Device::Cpu, Device::Gpu];

    fn label(self) -> Cow<'static, str> {
        match self {
            Device::Automatic => t!("tongue.device_automatic"),
            Device::Cpu => t!("tongue.device_cpu"),
            Device::Gpu => t!("tongue.device_gpu"),
        }
    }

    fn wire(self) -> TrainingDevice {
        match self {
            Device::Automatic => TrainingDevice::Auto,
            Device::Cpu => TrainingDevice::Cpu,
            Device::Gpu => TrainingDevice::Gpu,
        }
    }
}

/// The request in flight, for its button to show it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pending {
    StartRecording(CaptureMode),
    Capture(CaptureCommand),
    Activate(String),
    RenameModel(String),
    DeleteModel(String),
    InstallBuiltin,
    CancelBuiltin,
    InstallQftPlus,
    CancelQftPlus,
    RemoveQftPlus,
    Export(String),
    Import,
    Train,
    Cancel,
    Delete(String),
}

/// One reading of everything the section shows.
struct Snapshot {
    capture: anyhow::Result<CaptureStatus>,
    training: anyhow::Result<TrainingStatus>,
    lists: Option<(anyhow::Result<Vec<Recording>>, anyhow::Result<Models>)>,
}

fn read(client: &QuestProClient, lists: bool) -> Snapshot {
    Snapshot {
        capture: client.capture_status(),
        training: client.training_status(),
        lists: lists.then(|| (client.recordings(), client.models())),
    }
}

pub struct TongueTraining {
    daemon: Entity<QuestProState>,
    /// The mouth cameras, shown beside the prompt while recording.
    camera: Entity<CameraFeed>,
    launcher: Entity<Launcher>,
    capture: Option<CaptureStatus>,
    /// When `capture` arrived, to move the follow-the-dot dot between polls.
    capture_at: Instant,
    training: Option<TrainingStatus>,
    /// Why the daemon can't record or train, such as being too old.
    unavailable: Option<Notice>,
    recordings: Vec<Recording>,
    models: Option<Models>,
    /// Recordings left out of training. All others are ticked.
    unticked: HashSet<String>,
    /// The recording whose poses are shown for review.
    reviewing: Option<String>,
    /// Which of start, middle and end is shown for each reviewed pose.
    review_choice: HashMap<u64, usize>,
    /// Review images by recording and frame index.
    frames: HashMap<(String, u64), Arc<RenderImage>>,
    loading_frames: bool,
    confirm_delete: Option<String>,
    confirm_cancel: bool,
    /// Stop was pressed once; pressing it again ends the recording.
    confirm_stop: bool,
    /// Holds keyboard focus while recording, for the recording's keys.
    focus: FocusHandle,
    /// The step picked, or moved to after a recording or training; until
    /// then, the one that comes next.
    chosen_step: Option<Step>,
    show_advanced: bool,
    device: Device,
    /// The stereo pair, or the universal face model.
    architecture: TrainerArchitecture,
    name: Entity<InputState>,
    epochs: Entity<InputState>,
    /// The trained model being renamed, and its new name.
    renaming: Option<String>,
    rename: Entity<InputState>,
    /// The trained model asked to be deleted, waiting for a second yes.
    confirm_delete_model: Option<String>,
    voice: bool,
    speaker: Speaker,
    /// The last prompt read aloud, shown beside the pose in case it was
    /// missed.
    last_cue: Option<SharedString>,
    /// The training job whose outcome is already shown, and whether it was
    /// seen running.
    shown_job: Option<String>,
    seen_busy: Option<String>,
    pending: Option<Pending>,
    record_error: Option<Notice>,
    train_message: Option<Notice>,
    /// The training job that failed, for trying again and for its log.
    failed_job: Option<String>,
    /// Whether `train_message` is a training that finished and saved a model.
    just_trained: bool,
    model_message: Option<Notice>,
    /// When a success in `model_message` stops being shown.
    model_message_until: Option<Instant>,
    /// The export or import this app started, whose outcome isn't shown yet.
    awaited_transfer: Option<u64>,
    /// How the last export or import this app started went.
    transfer_message: Option<Notice>,
    /// The folder the last export made, for showing it.
    exported_to: Option<std::path::PathBuf>,
    list_message: Option<Notice>,
    watching: bool,
    lists_stale: bool,
    _poll: Task<()>,
    _action: Option<Task<()>>,
    _frames: Option<Task<()>>,
    _subscriptions: [Subscription; 3],
}

impl TongueTraining {
    pub fn new(
        daemon: Entity<QuestProState>,
        camera: Entity<CameraFeed>,
        launcher: Entity<Launcher>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("tongue.name_placeholder")));
        let epochs =
            cx.new(|cx| InputState::new(window, cx).placeholder(DEFAULT_EPOCHS.to_string()));
        let rename = cx.new(|cx| InputState::new(window, cx).placeholder(t!("tongue.model_name")));
        let poll = cx.spawn(async move |this, cx| loop {
            let Ok(plan) = this.update(cx, |section, cx| section.poll_plan(cx)) else {
                break;
            };
            if let Some((client, lists)) = plan {
                let snapshot = cx
                    .background_executor()
                    .spawn(async move { read(&client, lists) })
                    .await;
                if this
                    .update(cx, |section, cx| section.apply(snapshot, cx))
                    .is_err()
                {
                    break;
                }
            }
            let Ok(interval) = this.update(cx, |section, _| section.interval()) else {
                break;
            };
            cx.background_executor().timer(interval).await;
        });
        let subscriptions = [
            cx.observe(&daemon, |section, _, cx| {
                if section.watching {
                    cx.notify();
                }
            }),
            cx.observe(&camera, |section, _, cx| {
                if section.watching {
                    cx.notify();
                }
            }),
            cx.observe(&launcher, |section, _, cx| {
                if section.watching {
                    cx.notify();
                }
            }),
        ];
        Self {
            daemon,
            camera,
            launcher,
            capture: None,
            capture_at: Instant::now(),
            training: None,
            unavailable: None,
            recordings: Vec::new(),
            models: None,
            unticked: load_unticked(),
            reviewing: None,
            review_choice: HashMap::new(),
            frames: HashMap::new(),
            loading_frames: false,
            confirm_delete: None,
            confirm_cancel: false,
            confirm_stop: false,
            focus: cx.focus_handle(),
            chosen_step: None,
            show_advanced: false,
            device: Device::Automatic,
            architecture: TrainerArchitecture::StereoPair,
            name,
            epochs,
            renaming: None,
            rename,
            confirm_delete_model: None,
            voice: true,
            speaker: Speaker::default(),
            last_cue: None,
            shown_job: None,
            seen_busy: None,
            pending: None,
            record_error: None,
            train_message: None,
            failed_job: None,
            just_trained: false,
            model_message: None,
            model_message_until: None,
            awaited_transfer: None,
            transfer_message: None,
            exported_to: None,
            list_message: None,
            watching: false,
            lists_stale: true,
            _poll: poll,
            _action: None,
            _frames: None,
            _subscriptions: subscriptions,
        }
    }

    /// Polls while the page shows, and while a recording or training runs
    /// wherever the app is, so prompts are still read aloud.
    pub fn set_watching(&mut self, watching: bool, cx: &mut Context<Self>) {
        if self.watching == watching {
            return;
        }
        self.watching = watching;
        if watching {
            self.lists_stale = true;
            if self.voice {
                self.speaker.warm_up();
            }
        } else {
            self.close_review(cx);
        }
    }

    fn capture_active(&self) -> bool {
        self.capture.as_ref().is_some_and(|capture| capture.active)
    }

    fn training_busy(&self) -> bool {
        self.training.as_ref().is_some_and(|training| training.busy)
    }

    fn builtin_installing(&self) -> bool {
        self.training
            .as_ref()
            .and_then(|training| training.builtin.as_ref())
            .is_some_and(|builtin| builtin.installing)
    }

    fn builtin(&self) -> Option<&BuiltinStatus> {
        self.training
            .as_ref()
            .and_then(|training| training.builtin.as_ref())
    }

    /// QFT+'s face model; absent from daemons that can't install it.
    fn qftplus(&self) -> Option<&BuiltinStatus> {
        self.training
            .as_ref()
            .and_then(|training| training.qftplus.as_ref())
    }

    /// A model export or import running.
    fn transfer(&self) -> Option<&TransferStatus> {
        self.training
            .as_ref()
            .and_then(|training| training.transfer.as_ref())
            .filter(|transfer| transfer.busy)
    }

    fn transferring(&self) -> bool {
        self.transfer().is_some() || self.awaited_transfer.is_some()
    }

    fn online(&self, cx: &App) -> bool {
        *self.daemon.read(cx).connection() == Connection::Online
    }

    fn poll_plan(&mut self, cx: &App) -> Option<(Arc<QuestProClient>, bool)> {
        let wanted = self.watching
            || self.capture_active()
            || self.training_busy()
            || self.builtin_installing()
            || self.qftplus().is_some_and(|qftplus| qftplus.installing)
            || self.transferring();
        if !wanted || !self.online(cx) {
            return None;
        }
        let lists = std::mem::take(&mut self.lists_stale) && self.watching;
        if !self.watching {
            // Catch up when the page shows again.
            self.lists_stale = true;
        }
        Some((self.daemon.read(cx).client(), lists))
    }

    fn interval(&self) -> Duration {
        if self.capture_active() {
            RECORDING_INTERVAL
        } else {
            IDLE_INTERVAL
        }
    }

    fn apply(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        match snapshot.capture {
            Ok(capture) => {
                self.unavailable = None;
                self.show_capture(capture);
            }
            Err(error) => self.unavailable = Some(Notice::error("", &error)),
        }
        if let Ok(training) = snapshot.training {
            self.show_training(training);
        }
        if let Some((recordings, models)) = snapshot.lists {
            match recordings {
                Ok(recordings) => {
                    let before = self.unticked.len();
                    self.unticked
                        .retain(|id| recordings.iter().any(|recording| recording.id == *id));
                    if self.unticked.len() != before {
                        save_unticked(&self.unticked);
                    }
                    if self
                        .reviewing
                        .as_ref()
                        .is_some_and(|id| !recordings.iter().any(|r| r.id == *id))
                    {
                        self.close_review(cx);
                    }
                    self.recordings = recordings;
                    self.list_message = None;
                }
                Err(error) => {
                    self.list_message =
                        Some(Notice::error(&t!("tongue.couldnt_load_recordings"), &error))
                }
            }
            if let Ok(models) = models {
                self.models = Some(models);
            }
        }
        cx.notify();
    }

    fn show_capture(&mut self, capture: CaptureStatus) {
        let finished = self.capture_active() && !capture.active;
        if let Some(line) = crate::prompts::cue(self.capture.as_ref(), &capture) {
            self.say(&line);
        }
        if finished {
            self.confirm_stop = false;
            self.say(&if capture.message.starts_with("Recording complete") {
                t!("tongue.say_recording_complete")
            } else {
                t!("tongue.say_recording_stopped")
            });
            self.chosen_step = Some(Step::Record);
            self.lists_stale = true;
        }
        self.capture = Some(capture);
        self.capture_at = Instant::now();
    }

    fn show_training(&mut self, training: TrainingStatus) {
        if training.busy {
            self.seen_busy = training.id.clone();
        } else if let (Some(id), Some(progress)) = (&training.id, &training.progress) {
            if self.shown_job.as_ref() != Some(id) {
                self.shown_job = Some(id.clone());
                let seen = self.seen_busy.as_ref() == Some(id);
                self.just_trained = progress.stage == TrainingStage::Complete;
                self.failed_job = (progress.stage == TrainingStage::Failed).then(|| id.clone());
                if seen && progress.stage == TrainingStage::Failed {
                    self.say(&t!("tongue.say_training_failed"));
                }
                self.train_message = match progress.stage {
                    TrainingStage::Complete => {
                        if seen {
                            self.say(&t!("tongue.say_training_complete"));
                            // Straight on to trying the new model.
                            self.chosen_step = Some(Step::Try);
                        }
                        let switched = training.active_id == *id;
                        // The new model's row gives its recordings and time, and
                        // the panel why it isn't switched on.
                        let text = if switched {
                            t!("tongue.trained_switched_on")
                        } else if training.model_override {
                            t!("tongue.trained_not_switched_on")
                        } else {
                            t!("tongue.trained_saved")
                        };
                        Some(Notice::new(Tone::Good, text))
                    }
                    TrainingStage::Failed => {
                        let text = training_failure(&progress.message);
                        // A failure it can't put plainly already shows the whole
                        // message.
                        let notice = Notice::new(Tone::Problem, text.clone());
                        Some(if text.contains(progress.message.as_str()) {
                            notice
                        } else {
                            notice.details(progress.message.clone())
                        })
                    }
                    TrainingStage::Cancelled => {
                        Some(Notice::new(Tone::Off, t!("tongue.training_cancelled")))
                    }
                    _ => None,
                };
                // A job that ended before this app saw it is old news.
                if !seen {
                    self.train_message = None;
                    self.just_trained = false;
                    self.failed_job = None;
                }
                self.lists_stale = true;
            }
        }
        if let Some(models) = &mut self.models {
            models.active_id = training.active_id.clone();
        }
        if let Some(transfer) = &training.transfer {
            self.show_transfer(transfer);
        }
        self.training = Some(training);
    }

    /// Says how the export or import this app started went, once it's done.
    fn show_transfer(&mut self, transfer: &TransferStatus) {
        if transfer.busy || self.awaited_transfer != Some(transfer.serial) {
            return;
        }
        self.awaited_transfer = None;
        self.lists_stale = true;
        let recordings = recording_count(u64::from(transfer.recordings));
        self.exported_to = None;
        self.transfer_message = Some(match (transfer.kind, &transfer.error) {
            (TransferKind::Export, Some(error)) => {
                Notice::new(Tone::Problem, t!("tongue.export_failed", error = error))
            }
            (TransferKind::Import, Some(error)) => {
                Notice::new(Tone::Problem, t!("tongue.import_failed", error = error))
            }
            (TransferKind::Export, None) => {
                self.exported_to = transfer.folder.clone();
                let folder = transfer
                    .folder
                    .as_ref()
                    .map(|folder| folder.display().to_string())
                    .unwrap_or_default();
                Notice::new(
                    Tone::Good,
                    if transfer.recordings == 0 {
                        t!("tongue.exported_alone", folder = folder)
                    } else {
                        t!("tongue.exported", folder = folder, recordings = recordings)
                    },
                )
            }
            (TransferKind::Import, None) => Notice::new(
                Tone::Good,
                if transfer.recordings == 0 {
                    t!("tongue.imported_alone")
                } else {
                    t!("tongue.imported", recordings = recordings)
                },
            ),
        });
    }

    fn say(&mut self, text: &str) {
        if self.voice {
            self.speaker.say(text);
            if !self.speaker.unavailable() {
                self.last_cue = Some(SharedString::from(text.to_string()));
            }
        }
    }

    /// Runs one request off the UI thread, then `done` with its result.
    fn request<T: Send + 'static>(
        &mut self,
        pending: Pending,
        work: impl FnOnce(&QuestProClient) -> anyhow::Result<T> + Send + 'static,
        done: impl FnOnce(&mut Self, anyhow::Result<T>, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.pending.is_some() {
            return;
        }
        let client = self.daemon.read(cx).client();
        self.pending = Some(pending);
        cx.notify();
        self._action = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { work(&client) })
                .await;
            this.update(cx, |section, cx| {
                section.pending = None;
                done(section, result, cx);
                cx.notify();
            })
            .ok();
        }));
    }

    fn start_recording(&mut self, mode: CaptureMode, cx: &mut Context<Self>) {
        self.record_poses(mode, Vec::new(), cx);
    }

    /// Records only `poses` of `mode`, or all of it when there are none.
    fn record_poses(&mut self, mode: CaptureMode, poses: Vec<String>, cx: &mut Context<Self>) {
        self.record_error = None;
        self.request(
            Pending::StartRecording(mode),
            move |client| client.start_capture(mode, poses),
            Self::capture_done,
            cx,
        );
    }

    /// Records just the basic poses that give what training still lacks.
    fn record_missing(&mut self, lacking: &[&str], window: &mut Window, cx: &mut Context<Self>) {
        let poses = lacking
            .iter()
            .filter_map(|need| missing_pose(need))
            .map(String::from)
            .collect();
        window.focus(&self.focus, cx);
        self.go(Step::Record, cx);
        self.record_poses(CaptureMode::Core, poses, cx);
    }

    /// Records just the basic poses that teach the cheek puffs.
    fn record_cheeks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.go(Step::Record, cx);
        let poses = CHEEK_POSES.map(String::from).to_vec();
        self.record_poses(CaptureMode::Core, poses, cx);
    }

    /// Stop asks first, since it can't be undone: a second press stops.
    fn stop_pressed(&mut self, cx: &mut Context<Self>) {
        if self.confirm_stop {
            self.confirm_stop = false;
            self.capture_command(CaptureCommand::Stop, cx);
        } else {
            self.confirm_stop = true;
            self.say(&t!("tongue.say_stop_recording"));
            cx.notify();
        }
    }

    fn keep_recording(&mut self, cx: &mut Context<Self>) {
        self.confirm_stop = false;
        cx.notify();
    }

    fn capture_command(&mut self, command: CaptureCommand, cx: &mut Context<Self>) {
        if command != CaptureCommand::Stop {
            self.confirm_stop = false;
        }
        self.request(
            Pending::Capture(command),
            move |client| client.capture_command(command),
            Self::capture_done,
            cx,
        );
    }

    fn capture_done(&mut self, result: anyhow::Result<CaptureStatus>, _: &mut Context<Self>) {
        match result {
            Ok(capture) => {
                self.record_error = None;
                self.show_capture(capture);
            }
            Err(error) => self.record_error = Some(Notice::error("", &error)),
        }
    }

    fn set_voice(&mut self, on: bool, cx: &mut Context<Self>) {
        self.voice = on;
        if on {
            // Say where the recording is, once it's switched back on.
            let pose = self
                .capture
                .as_ref()
                .filter(|capture| capture.active)
                .and_then(|capture| capture.pose.clone());
            match pose {
                Some(pose) => self.say(&pose),
                None => self.speaker.warm_up(),
            }
        } else {
            self.speaker.hush();
        }
        cx.notify();
    }

    /// Ways on after a failed training: again on the CPU, or its log.
    fn failure_actions(&self, busy: bool, cx: &Context<Self>) -> Option<AnyElement> {
        let id = self.failed_job.as_ref()?;
        let log = vrft_gui_core::paths::config_file()
            .and_then(|config| config.parent().map(std::path::Path::to_path_buf))
            .map(|root| {
                root.join(".local/tongue-models")
                    .join(id)
                    .join("training.log")
            })
            .filter(|log| log.is_file());
        Some(
            h_flex()
                .gap_2()
                .when(self.device != Device::Cpu, |row| {
                    row.child(
                        Button::new("train-on-cpu")
                            .outline()
                            .small()
                            .label(t!("tongue.try_again_on_cpu"))
                            .tooltip(t!("tongue.try_again_on_cpu_tooltip"))
                            .disabled(busy || self.pending.is_some())
                            .on_click(cx.listener(|section, _, _, cx| {
                                section.device = Device::Cpu;
                                section.start_training(cx);
                            })),
                    )
                })
                .when_some(log, |row, log| {
                    row.child(
                        Button::new("open-training-log")
                            .ghost()
                            .small()
                            .label(t!("tongue.open_log"))
                            .on_click(move |_, _, cx| cx.open_with_system(&log)),
                    )
                })
                .into_any_element(),
        )
    }

    /// Says a sample prompt, so people can check the volume before putting
    /// the headset on.
    fn test_voice(&mut self, cx: &mut Context<Self>) {
        self.speaker.say(&t!("tongue.say_test_voice"));
        cx.notify();
    }

    /// The read-aloud switch, a way to hear it, and why it's silent if it is.
    fn voice_controls(&self, id: &'static str, cx: &Context<Self>) -> AnyElement {
        v_flex()
            .gap_2()
            .child(self.voice_row(id, cx))
            .children(self.voice_notice())
            .into_any_element()
    }

    /// The read-aloud switch and a way to hear it.
    fn voice_row(&self, id: &'static str, cx: &Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_3()
            .items_center()
            .flex_wrap()
            .child(
                Switch::new(id)
                    .small()
                    .label(SharedString::from(t!("tongue.read_prompts_aloud")))
                    .checked(self.voice)
                    .on_change(cx.listener(|section, on: &bool, _, cx| section.set_voice(*on, cx))),
            )
            .child(
                Button::new(SharedString::from(format!("{id}-test")))
                    .ghost()
                    .small()
                    .icon(IconName::Volume2)
                    .label(t!("tongue.test_voice"))
                    .disabled(self.speaker.unavailable())
                    .on_click(cx.listener(|section, _, _, cx| section.test_voice(cx))),
            )
    }

    /// Why prompts aren't read aloud when they should be.
    fn voice_notice(&self) -> Option<Notice> {
        (self.voice && self.speaker.unavailable())
            .then(|| Notice::new(Tone::Problem, t!("tongue.cant_read_aloud")))
    }

    fn activate(&mut self, id: String, cx: &mut Context<Self>) {
        self.model_message = None;
        self.model_message_until = None;
        let request = id.clone();
        self.request(
            Pending::Activate(id.clone()),
            move |client| client.activate_model(&request),
            move |section, result, _| match result {
                Ok(()) => {
                    if let Some(models) = &mut section.models {
                        models.active_id = id;
                    }
                    section.model_message = Some(Notice::new(Tone::Good, t!("tongue.switched")));
                    section.model_message_until = Some(Instant::now() + SUCCESS_SHOWN_FOR);
                }
                Err(error) => section.model_message = Some(Notice::error("", &error)),
            },
            cx,
        );
    }

    fn start_rename(
        &mut self,
        id: String,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.confirm_delete_model = None;
        self.renaming = Some(id);
        self.rename
            .update(cx, |input, cx| input.set_value(name, window, cx));
        cx.notify();
    }

    fn finish_rename(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.renaming.clone() else {
            return;
        };
        let name = self.rename.read(cx).value().trim().to_string();
        if name.is_empty() {
            self.model_message = Some(Notice::new(Tone::Problem, t!("tongue.give_a_name")));
            cx.notify();
            return;
        }
        self.model_message = None;
        self.request(
            Pending::RenameModel(id.clone()),
            move |client| client.rename_model(&id, &name),
            move |section, result, _| match result {
                Ok(renamed) => {
                    section.renaming = None;
                    if let Some(model) = section
                        .models
                        .iter_mut()
                        .flat_map(|models| &mut models.models)
                        .find(|model| model.id == renamed.id)
                    {
                        *model = renamed;
                    }
                }
                Err(error) => {
                    section.model_message =
                        Some(Notice::error(&t!("tongue.couldnt_rename"), &error))
                }
            },
            cx,
        );
    }

    fn delete_model(&mut self, id: String, cx: &mut Context<Self>) {
        self.model_message = None;
        let request = id.clone();
        self.request(
            Pending::DeleteModel(id.clone()),
            move |client| client.delete_model(&request),
            move |section, result, _| {
                section.confirm_delete_model = None;
                match result {
                    Ok(()) => {
                        if let Some(models) = &mut section.models {
                            models.models.retain(|model| model.id != id);
                        }
                        section.model_message = Some(Notice::new(Tone::Good, t!("tongue.deleted")));
                        section.model_message_until = Some(Instant::now() + SUCCESS_SHOWN_FOR);
                    }
                    Err(error) => {
                        section.model_message =
                            Some(Notice::error(&t!("tongue.couldnt_delete_it"), &error))
                    }
                }
            },
            cx,
        );
    }

    fn install_builtin(&mut self, cx: &mut Context<Self>) {
        self.model_message = None;
        self.request(
            Pending::InstallBuiltin,
            |client| client.install_builtin(),
            |section, result, _| match result {
                Ok(()) => {
                    if let Some(builtin) = section
                        .training
                        .as_mut()
                        .and_then(|training| training.builtin.as_mut())
                    {
                        builtin.installing = true;
                        builtin.error = None;
                        builtin.cancelled = false;
                        builtin.stage = Some(BuiltinStage::Connecting);
                    }
                }
                Err(error) => {
                    section.model_message =
                        Some(Notice::error(&t!("tongue.couldnt_start_download"), &error))
                }
            },
            cx,
        );
    }

    fn cancel_builtin(&mut self, cx: &mut Context<Self>) {
        self.request(
            Pending::CancelBuiltin,
            |client| client.cancel_builtin(),
            |section, result, _| {
                if let Err(error) = result {
                    section.model_message = Some(Notice::error("", &error));
                }
            },
            cx,
        );
    }

    fn install_qftplus(&mut self, cx: &mut Context<Self>) {
        self.model_message = None;
        self.request(
            Pending::InstallQftPlus,
            |client| client.install_qftplus(),
            |section, result, _| match result {
                Ok(()) => {
                    if let Some(qftplus) = section
                        .training
                        .as_mut()
                        .and_then(|training| training.qftplus.as_mut())
                    {
                        qftplus.installing = true;
                        qftplus.error = None;
                        qftplus.cancelled = false;
                        qftplus.stage = Some(BuiltinStage::Connecting);
                    }
                }
                Err(error) => {
                    section.model_message =
                        Some(Notice::error(&t!("tongue.couldnt_start_download"), &error))
                }
            },
            cx,
        );
    }

    fn cancel_qftplus(&mut self, cx: &mut Context<Self>) {
        self.request(
            Pending::CancelQftPlus,
            |client| client.cancel_qftplus(),
            |section, result, _| {
                if let Err(error) = result {
                    section.model_message = Some(Notice::error("", &error));
                }
            },
            cx,
        );
    }

    fn remove_qftplus(&mut self, cx: &mut Context<Self>) {
        self.model_message = None;
        self.request(
            Pending::RemoveQftPlus,
            |client| client.remove_qftplus(),
            |section, result, _| match result {
                Ok(()) => {
                    if let Some(qftplus) = section
                        .training
                        .as_mut()
                        .and_then(|training| training.qftplus.as_mut())
                    {
                        qftplus.installed = false;
                    }
                    section.model_message = Some(Notice::new(Tone::Good, t!("tongue.removed")));
                    section.model_message_until = Some(Instant::now() + SUCCESS_SHOWN_FOR);
                }
                Err(error) => {
                    section.model_message =
                        Some(Notice::error(&t!("tongue.couldnt_remove"), &error))
                }
            },
            cx,
        );
    }

    /// The QFTPlus Model, the base model: offered for download while it
    /// isn't in place, and its download while it runs; as the card's main
    /// action while it's the model in use.
    fn qftplus_panel(&self, primary: bool, cx: &Context<Self>) -> Option<AnyElement> {
        let qftplus = self.qftplus()?;
        if qftplus.installing {
            return Some(download_progress(
                qftplus,
                Package::QftPlus,
                self.pair_installed(),
                self.pending.is_some(),
                cx,
            ));
        }
        if qftplus.installed {
            return None;
        }
        let failed = qftplus.error.is_some();
        let size = qftplus.download_megabytes.unwrap_or(334);
        let state = match &qftplus.error {
            Some(error) => t!("tongue.download_failed", error = error),
            None if qftplus.cancelled => t!("tongue.download_cancelled", size = size),
            None => t!("tongue.not_downloaded_size", size = size),
        };
        let button = Button::new("install-qftplus")
            .when(primary, |button| button.primary())
            .small()
            .label(if failed {
                t!("tongue.try_again")
            } else {
                t!("tongue.download")
            })
            .tooltip(t!("tongue.download_tooltip"))
            .loading(self.pending == Some(Pending::InstallQftPlus))
            // Training's download fetches the same pair.
            .disabled(self.pending.is_some() || self.builtin_installing())
            .on_click(cx.listener(|section, _, _, cx| section.install_qftplus(cx)));
        Some(
            h_flex()
                .w_full()
                .flex_wrap()
                .items_center()
                .gap_3()
                .px_3p5()
                .py_3()
                .rounded(px(10.))
                .border_1()
                .border_color(if failed {
                    palette::signal_line()
                } else {
                    palette::line_strong()
                })
                .bg(if failed {
                    palette::signal_bg()
                } else {
                    palette::inset()
                })
                .child(
                    div()
                        .flex_none()
                        .text_color(if failed {
                            palette::signal()
                        } else {
                            palette::text_3()
                        })
                        .child(Icon::new(IconName::Download).size(px(16.))),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w(px(160.))
                        .gap_0p5()
                        .child(
                            div()
                                .text_sm()
                                .font_medium()
                                .child(t!("tongue.qftplus_model")),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(if failed {
                                    palette::signal_text()
                                } else {
                                    palette::text_3()
                                })
                                .child(state),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(palette::text_3())
                                .child(t!("tongue.qftplus_lead")),
                        ),
                )
                .child(button)
                .into_any_element(),
        )
    }

    /// Offers to download what training needs while it isn't in place, and
    /// shows the download while it runs; as the view's main action when
    /// nothing else can go on without it.
    fn training_files_panel(&self, primary: bool, cx: &Context<Self>) -> Option<AnyElement> {
        let builtin = self.builtin()?;
        if builtin.installed && !builtin.examples_missing {
            return None;
        }
        if builtin.installing {
            return Some(download_progress(
                builtin,
                Package::TrainingFiles,
                builtin.installed,
                self.pending.is_some(),
                cx,
            ));
        }
        let failed = builtin.error.is_some();
        let size = builtin.download_megabytes.unwrap_or(263);
        let state = match &builtin.error {
            Some(error) => t!("tongue.download_failed", error = error),
            None if builtin.cancelled => t!("tongue.download_cancelled", size = size),
            None => t!("tongue.not_downloaded_size", size = size),
        };
        // With the pair in place, only the examples are left to download.
        let what = if builtin.installed {
            t!("tongue.training_examples")
        } else {
            t!("tongue.training_files")
        };
        // One row: what it is and how it stands, with its button beside it,
        // wrapping under it when the card is narrow.
        Some(
            h_flex()
                .w_full()
                .flex_wrap()
                .items_center()
                .gap_3()
                .px_3p5()
                .py_3()
                .rounded(px(10.))
                .border_1()
                .border_color(if failed {
                    palette::signal_line()
                } else {
                    palette::line_strong()
                })
                .bg(if failed {
                    palette::signal_bg()
                } else {
                    palette::inset()
                })
                .child(
                    div()
                        .flex_none()
                        .text_color(if failed {
                            palette::signal()
                        } else {
                            palette::text_3()
                        })
                        .child(Icon::new(IconName::Download).size(px(16.))),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w(px(160.))
                        .gap_0p5()
                        .child(div().text_sm().font_medium().child(what))
                        .child(
                            div()
                                .text_xs()
                                .text_color(if failed {
                                    palette::signal_text()
                                } else {
                                    palette::text_3()
                                })
                                .child(state),
                        ),
                )
                .child(
                    Button::new("install-builtin")
                        .when(primary, |button| button.primary())
                        .small()
                        .label(if failed {
                            t!("tongue.try_again")
                        } else {
                            t!("tongue.download")
                        })
                        .tooltip(t!("tongue.download_tooltip"))
                        .loading(self.pending == Some(Pending::InstallBuiltin))
                        // The QFTPlus Model's download fetches the same pair.
                        .disabled(
                            self.pending.is_some()
                                || self.qftplus().is_some_and(|qftplus| qftplus.installing),
                        )
                        .on_click(cx.listener(|section, _, _, cx| section.install_builtin(cx))),
                )
                .into_any_element(),
        )
    }

    /// Asks where to export trained model `id`, then starts the export.
    fn export_model(&mut self, id: String, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t!("tongue.export_prompt").into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(folder) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |section, cx| {
                section.start_transfer(
                    Pending::Export(id.clone()),
                    move |client| client.export_model(&id, folder),
                    cx,
                )
            })
            .ok();
        })
        .detach();
    }

    /// Asks for an exported model's folder, or a zip of one, then starts
    /// importing it.
    fn import_model(&mut self, zip: bool, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: zip,
            directories: !zip,
            multiple: false,
            prompt: Some(if zip {
                t!("tongue.import_zip_prompt").into()
            } else {
                t!("tongue.import_folder_prompt").into()
            }),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |section, cx| {
                section.start_transfer(Pending::Import, move |client| client.import_model(path), cx)
            })
            .ok();
        })
        .detach();
    }

    fn start_transfer(
        &mut self,
        pending: Pending,
        work: impl FnOnce(&QuestProClient) -> anyhow::Result<u64> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.transfer_message = None;
        self.exported_to = None;
        let export = matches!(pending, Pending::Export(_));
        self.request(
            pending,
            work,
            move |section, result, _| match result {
                Ok(serial) => section.awaited_transfer = Some(serial),
                Err(error) => {
                    section.transfer_message = Some(Notice::error(
                        &if export {
                            t!("tongue.couldnt_export")
                        } else {
                            t!("tongue.couldnt_import")
                        },
                        &error,
                    ))
                }
            },
            cx,
        );
    }

    /// An export or import running, or how the last one this app started
    /// went.
    fn transfer_panel(&self) -> Option<AnyElement> {
        if let Some(transfer) = self.transfer() {
            let percent = (transfer.fraction.unwrap_or(0.) * 100.).round();
            let text = match transfer.kind {
                TransferKind::Export => t!("tongue.exporting", percent = percent),
                TransferKind::Import => t!("tongue.importing", percent = percent),
            };
            return Some(
                v_flex()
                    .w_full()
                    .gap_2()
                    .child(Notice::new(Tone::Waiting, text))
                    .child(Meter::new(transfer.fraction.unwrap_or(0.), palette::text()))
                    .into_any_element(),
            );
        }
        let message = self.transfer_message.clone()?;
        Some(
            v_flex()
                .w_full()
                .gap_2()
                .child(message)
                .children(self.exported_to.clone().map(|folder| {
                    h_flex().child(
                        Button::new("show-export")
                            .small()
                            .icon(IconName::FolderOpen)
                            .label(t!("tongue.show_folder"))
                            .on_click(move |_, _, cx| cx.reveal_path(&folder)),
                    )
                }))
                .into_any_element(),
        )
    }

    fn ticked(&self) -> Vec<&Recording> {
        self.recordings
            .iter()
            .filter(|recording| recording.error.is_none() && !self.unticked.contains(&recording.id))
            .collect()
    }

    fn tick(&mut self, id: String, ticked: bool, cx: &mut Context<Self>) {
        if ticked {
            self.unticked.remove(&id);
        } else {
            self.unticked.insert(id);
        }
        save_unticked(&self.unticked);
        cx.notify();
    }

    fn start_training(&mut self, cx: &mut Context<Self>) {
        let typed_name = self.name.read(cx).value().trim().to_string();
        let name = if typed_name.is_empty() {
            default_name(Local::now())
        } else {
            typed_name.chars().take(100).collect()
        };
        let typed_epochs = self.epochs.read(cx).value().trim().to_string();
        let epochs = if typed_epochs.is_empty() {
            DEFAULT_EPOCHS
        } else {
            match typed_epochs.parse::<u32>() {
                Ok(epochs) if (1..=60).contains(&epochs) => epochs,
                _ => {
                    self.train_message =
                        Some(Notice::new(Tone::Problem, t!("tongue.passes_invalid")));
                    cx.notify();
                    return;
                }
            }
        };
        let request = TrainRequest {
            name,
            recordings: self.ticked().iter().map(|r| r.id.clone()).collect(),
            device: self.device.wire(),
            epochs,
            architecture: self.architecture,
        };
        self.train_message = None;
        self.failed_job = None;
        self.just_trained = false;
        self.request(
            Pending::Train,
            move |client| client.start_training(&request),
            |section, result, _| match result {
                Ok(()) => {
                    // Show it running straight away; the next poll fills in.
                    if let Some(training) = &mut section.training {
                        training.busy = true;
                        training.progress = None;
                    }
                }
                Err(error) => {
                    section.train_message =
                        Some(Notice::error(&t!("tongue.couldnt_start_training"), &error))
                }
            },
            cx,
        );
    }

    fn cancel_training(&mut self, cx: &mut Context<Self>) {
        self.confirm_cancel = false;
        self.request(
            Pending::Cancel,
            |client| client.cancel_training(),
            |section, result, _| {
                if let Err(error) = result {
                    section.train_message = Some(Notice::error("", &error));
                }
            },
            cx,
        );
    }

    fn delete(&mut self, id: String, cx: &mut Context<Self>) {
        self.confirm_delete = None;
        let request = id.clone();
        self.request(
            Pending::Delete(id.clone()),
            move |client| client.delete_recording(&request),
            move |section, result, cx| {
                match result {
                    Ok(()) => {
                        if section.unticked.remove(&id) {
                            save_unticked(&section.unticked);
                        }
                        section.recordings.retain(|recording| recording.id != id);
                        if section.reviewing.as_ref() == Some(&id) {
                            section.close_review(cx);
                        }
                    }
                    Err(error) => {
                        section.list_message =
                            Some(Notice::error(&t!("tongue.couldnt_delete"), &error))
                    }
                }
                section.lists_stale = true;
            },
            cx,
        );
    }

    fn toggle_review(&mut self, id: String, cx: &mut Context<Self>) {
        let open = self.reviewing.as_ref() != Some(&id);
        self.close_review(cx);
        if open {
            self.reviewing = Some(id);
            self.load_frames(cx);
        }
        cx.notify();
    }

    fn close_review(&mut self, cx: &mut Context<Self>) {
        self.reviewing = None;
        self.review_choice.clear();
        self._frames = None;
        self.loading_frames = false;
        for (_, image) in self.frames.drain() {
            cx.drop_image(image, None);
        }
    }

    fn choose_frame(&mut self, step: u64, choice: usize, cx: &mut Context<Self>) {
        self.review_choice.insert(step, choice);
        self.load_frames(cx);
        cx.notify();
    }

    /// Downloads the review images still missing, one at a time.
    fn load_frames(&mut self, cx: &mut Context<Self>) {
        if self.loading_frames {
            return;
        }
        let Some(recording) = self
            .reviewing
            .as_ref()
            .and_then(|id| self.recordings.iter().find(|r| r.id == *id))
        else {
            return;
        };
        let id = recording.id.clone();
        let wanted: Vec<u64> = recording
            .poses
            .iter()
            .map(|pose| pose.indices[self.review_choice.get(&pose.step).copied().unwrap_or(1)])
            .filter(|index| !self.frames.contains_key(&(id.clone(), *index)))
            .collect();
        if wanted.is_empty() {
            return;
        }
        let client = self.daemon.read(cx).client();
        self.loading_frames = true;
        self._frames = Some(cx.spawn(async move |this, cx| {
            // A few at a time: a recording can have two dozen poses.
            let mut loading = std::collections::VecDeque::new();
            let mut wanted = wanted.into_iter();
            loop {
                while loading.len() < FRAMES_AT_ONCE {
                    let Some(index) = wanted.next() else {
                        break;
                    };
                    let (client, id_for_request) = (client.clone(), id.clone());
                    loading.push_back((
                        index,
                        cx.background_executor().spawn(async move {
                            client
                                .recorded_frame(&id_for_request, index)
                                .map(|frame| crate::live::decode(frame).1)
                        }),
                    ));
                }
                let Some((index, task)) = loading.pop_front() else {
                    break;
                };
                let image = task.await;
                let kept = this.update(cx, |section, cx| {
                    if section.reviewing.as_ref() != Some(&id) {
                        if let Ok(image) = image {
                            cx.drop_image(image, None);
                        }
                        return false;
                    }
                    match image {
                        Ok(image) => {
                            section.frames.insert((id.clone(), index), image);
                        }
                        Err(error) => {
                            section.list_message =
                                Some(Notice::error(&t!("tongue.couldnt_show_frame"), &error))
                        }
                    }
                    cx.notify();
                    true
                });
                if !matches!(kept, Ok(true)) {
                    return;
                }
            }
            this.update(cx, |section, cx| {
                section.loading_frames = false;
                // Someone may have picked another frame meanwhile.
                section.load_frames(cx);
            })
            .ok();
        }));
    }

    /// Ticks or unticks one pose of the reviewed recording for training.
    fn include_pose(&mut self, id: String, step: u64, include: bool, cx: &mut Context<Self>) {
        let Some(recording) = self.recordings.iter_mut().find(|r| r.id == id) else {
            return;
        };
        let Some(pose) = recording.poses.iter_mut().find(|pose| pose.step == step) else {
            return;
        };
        pose.excluded = !include;
        let excluded: Vec<u64> = recording
            .poses
            .iter()
            .filter(|pose| pose.excluded)
            .map(|pose| pose.step)
            .collect();
        let client = self.daemon.read(cx).client();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { client.review_recording(&id, &excluded) })
                .await;
            this.update(cx, |section, cx| {
                if let Err(error) = result {
                    section.list_message = Some(Notice::error(&t!("tongue.couldnt_save"), &error));
                }
                // Coverage changes with the poses left out.
                section.lists_stale = true;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The personal models trained here, newest first.
    fn trained_models(&self) -> Vec<&SavedModel> {
        let mut models: Vec<_> = self
            .models
            .iter()
            .flat_map(|models| &models.models)
            .filter(|model| model.id != "demo")
            .collect();
        models.sort_by_key(|model| Reverse(created_at(&model.id)));
        models
    }

    fn active_id(&self) -> &str {
        self.models
            .as_ref()
            .map_or("demo", |models| models.active_id.as_str())
    }

    /// The personal model in use, when it isn't the QFTPlus Model.
    fn active_model(&self) -> Option<&SavedModel> {
        let active = self.active_id();
        self.trained_models()
            .into_iter()
            .find(|model| model.id == active)
    }

    /// Whether what training needs, the mouth-camera pair it starts from
    /// and the training examples it mixes in, still needs downloading.
    fn training_files_missing(&self) -> bool {
        self.builtin()
            .is_some_and(|builtin| !builtin.installed || builtin.examples_missing)
    }

    /// Whether the mouth-camera pair is in place, which the QFTPlus Model's
    /// download fetches before QFT+'s own model.
    fn pair_installed(&self) -> bool {
        self.builtin().is_none_or(|builtin| builtin.installed)
    }

    /// Whether the QFTPlus Model, which is used unless a trained model is,
    /// still needs downloading.
    fn qftplus_missing(&self) -> bool {
        self.qftplus().is_some_and(|qftplus| !qftplus.installed)
    }

    fn recorded(&self) -> bool {
        self.recordings
            .iter()
            .any(|recording| recording.error.is_none())
    }

    /// Whether the ticked recordings cover everything training needs.
    fn enough(&self) -> bool {
        let ticked = self.ticked();
        !ticked.is_empty() && missing(&ticked).is_empty()
    }

    /// The step that comes next: record until there's enough to train, train
    /// until there's a model, then try it.
    fn guided_step(&self) -> Step {
        if self.training_busy() {
            Step::Train
        } else if !self.trained_models().is_empty() {
            Step::Try
        } else if self.enough() {
            Step::Train
        } else {
            Step::Record
        }
    }

    fn go(&mut self, step: Step, cx: &mut Context<Self>) {
        self.chosen_step = Some(step);
        cx.notify();
    }

    /// Whether a step is done: enough recorded, a model trained, and one
    /// of those in use.
    fn step_done(&self, step: Step) -> bool {
        match step {
            Step::Record => self.enough(),
            Step::Train => !self.training_busy() && !self.trained_models().is_empty(),
            Step::Try => self.active_model().is_some(),
        }
    }

    /// How far training is while it runs, as a percentage.
    fn training_percent(&self) -> Option<f32> {
        self.training
            .as_ref()
            .filter(|training| training.busy)
            .map(|training| {
                let fraction = training
                    .progress
                    .as_ref()
                    .and_then(|progress| progress.fraction)
                    .unwrap_or(0.);
                fraction.clamp(0., 1.) * 100.
            })
    }

    /// Record, Train and Test as a row of numbered steps: the one shown is
    /// outlined and bright, and each is ticked once it's done.
    fn tabs(&self, shown: Step, cx: &Context<Self>) -> impl IntoElement {
        h_flex().id("training-tabs").w_full().gap_2().children(
            Step::ALL.into_iter().enumerate().map(|(index, step)| {
                let percent = self.training_percent().filter(|_| step == Step::Train);
                step_tab(index, step, step == shown, self.step_done(step), percent)
                    .on_click(cx.listener(move |section, _, _, cx| section.go(step, cx)))
            }),
        )
    }

    /// The header's pill: the model in use, or how training or the model
    /// stands when that matters more.
    fn header_pill(&self, cx: &App) -> AnyElement {
        if self.training_busy() {
            return StatusPill::new(Tone::Waiting, t!("tongue.training")).into_any_element();
        }
        let model = summary::tongue_model(self.daemon.read(cx).status());
        // A model that can't run needs the player; the Mouth page says why.
        if model.tone == Tone::Problem {
            return StatusPill::new(model.tone, model.value).into_any_element();
        }
        let name = self
            .active_model()
            .map_or_else(|| t!("tongue.qftplus_model").to_string(), model_name);
        h_flex()
            .flex_none()
            .gap_2()
            .h(px(28.))
            .px_3()
            .rounded_full()
            .border_1()
            .border_color(palette::line_strong())
            .text_size(px(12.5))
            .text_color(palette::text_2())
            .child(t!("tongue.in_use"))
            .child(div().text_color(palette::text()).child(name))
            .into_any_element()
    }

    /// The models to choose between: the QFTPlus Model, then those trained
    /// here, newest first.
    fn models_panel(&self, cx: &Context<Self>) -> AnyElement {
        let locked = self.training_busy() || self.capture_active();
        let override_set = self
            .training
            .as_ref()
            .is_some_and(|training| training.model_override);
        let active = self.active_id();
        let now = Local::now();
        // Why "Use" can't be pressed, when it can't.
        let use_blocked = if override_set {
            Some(t!("tongue.override_picks_model"))
        } else if locked {
            Some(t!("tongue.finish_first"))
        } else {
            None
        };
        // A trained model in use doesn't need the QFTPlus Model, so it isn't
        // offered while it isn't downloaded; the panel above offers it.
        let base_row = !self.qftplus_missing() || self.active_model().is_none();
        let qftplus_installed = self.qftplus().is_some_and(|qftplus| qftplus.installed);
        let trained_count = self.trained_models().len();
        let transferring = self.transferring();
        let rows = base_row
            .then(|| {
                (
                    "demo".to_string(),
                    t!("tongue.qftplus_model").to_string(),
                    t!("tongue.not_trained_on_you").to_string(),
                )
            })
            .into_iter()
            .chain(self.trained_models().into_iter().map(|model| {
                (
                    model.id.clone(),
                    model_name(model),
                    model_detail(model, now),
                )
            }))
            .enumerate()
            .map(|(index, (id, name, detail))| {
                let trained = id != "demo";
                let renaming = self.renaming.as_deref() == Some(id.as_str());
                let confirming = self.confirm_delete_model.as_deref() == Some(id.as_str());
                let in_use = id == active;
                // The QFTPlus Model is "in use" only once it's there to use.
                let missing = id == "demo" && self.qftplus_missing();
                let activating = self.pending == Some(Pending::Activate(id.clone()));
                h_flex()
                    .gap_3()
                    .min_h(px(56.))
                    .px(px(18.))
                    .py(px(10.))
                    .border_t_1()
                    .border_color(palette::line_soft())
                    .when(in_use, |row| row.bg(palette::raised()))
                    .child(if renaming {
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .child(div().flex_1().child(Input::new(&self.rename).small()))
                            .child(
                                Button::new(("save-name", index))
                                    .primary()
                                    .small()
                                    .label(t!("tongue.save"))
                                    .loading(self.pending == Some(Pending::RenameModel(id.clone())))
                                    .on_click(
                                        cx.listener(|section, _, _, cx| section.finish_rename(cx)),
                                    ),
                            )
                            .child(
                                Button::new(("cancel-name", index))
                                    .ghost()
                                    .small()
                                    .label(t!("tongue.cancel"))
                                    .on_click(cx.listener(|section, _, _, cx| {
                                        section.renaming = None;
                                        cx.notify();
                                    })),
                            )
                            .into_any_element()
                    } else {
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_medium()
                                    .truncate()
                                    .child(name.clone()),
                            )
                            .child(div().text_xs().text_color(palette::text_3()).child(detail))
                            .into_any_element()
                    })
                    .when(trained && !renaming, |row| {
                        let rename_id = id.clone();
                        let delete_id = id.clone();
                        let export_id = id.clone();
                        let busy = locked || self.pending.is_some() || transferring;
                        // With nothing else to switch to, the model in use can go
                        // too; the QFTPlus Model is then offered again.
                        let deletable = !in_use || (self.qftplus_missing() && trained_count == 1);
                        row.child(
                            h_flex()
                                .gap_1()
                                .flex_none()
                                .child(
                                    Button::new(("export-model", index))
                                        .ghost()
                                        .small()
                                        .icon(IconName::FolderOutput)
                                        .tooltip(t!("tongue.export"))
                                        .loading(self.pending == Some(Pending::Export(id.clone())))
                                        .disabled(busy)
                                        .on_click(cx.listener(move |section, _, _, cx| {
                                            section.export_model(export_id.clone(), cx)
                                        })),
                                )
                                .child(
                                    Button::new(("rename-model", index))
                                        .ghost()
                                        .small()
                                        .icon(IconName::Pencil)
                                        .tooltip(t!("tongue.rename"))
                                        .disabled(busy)
                                        .on_click(cx.listener(move |section, _, window, cx| {
                                            section.start_rename(
                                                rename_id.clone(),
                                                name.clone(),
                                                window,
                                                cx,
                                            )
                                        })),
                                )
                                .child(if confirming {
                                    Button::new(("confirm-delete-model", index))
                                        .danger()
                                        .outline()
                                        .small()
                                        .label(t!("tongue.delete"))
                                        .loading(
                                            self.pending == Some(Pending::DeleteModel(id.clone())),
                                        )
                                        .on_click(cx.listener(move |section, _, _, cx| {
                                            section.delete_model(delete_id.clone(), cx)
                                        }))
                                } else {
                                    Button::new(("delete-model", index))
                                        .ghost()
                                        .small()
                                        .icon(IconName::Trash)
                                        .tooltip(if deletable {
                                            t!("tongue.delete")
                                        } else {
                                            t!("tongue.switch_before_deleting")
                                        })
                                        .disabled(busy || !deletable)
                                        .on_click(cx.listener(move |section, _, _, cx| {
                                            section.renaming = None;
                                            section.confirm_delete_model = Some(delete_id.clone());
                                            cx.notify();
                                        }))
                                }),
                        )
                    })
                    .when(!trained && qftplus_installed, |row| {
                        row.child(
                            Button::new("remove-qftplus")
                                .ghost()
                                .small()
                                .icon(IconName::Trash)
                                .tooltip(t!("tongue.qftplus_remove_tooltip"))
                                .loading(self.pending == Some(Pending::RemoveQftPlus))
                                .disabled(locked || self.pending.is_some())
                                .on_click(
                                    cx.listener(|section, _, _, cx| section.remove_qftplus(cx)),
                                ),
                        )
                    })
                    .child(if in_use && missing {
                        StatusPill::new(Tone::Waiting, t!("tongue.not_downloaded"))
                            .into_any_element()
                    } else if in_use {
                        StatusPill::new(Tone::Good, t!("tongue.in_use")).into_any_element()
                    } else {
                        Button::new(("use-model", index))
                            .small()
                            .label(t!("tongue.use"))
                            .loading(activating)
                            .when_some(use_blocked.clone(), |button, reason| button.tooltip(reason))
                            .disabled(locked || override_set || self.pending.is_some())
                            .on_click(cx.listener(move |section, _, _, cx| {
                                section.activate(id.clone(), cx)
                            }))
                            .into_any_element()
                    })
                    .into_any_element()
            });
        let model_message = self.model_message.clone().filter(|_| {
            self.model_message_until
                .is_none_or(|until| Instant::now() < until)
        });
        let import_blocked = self.pending.is_some() || transferring;
        let importing = self.pending == Some(Pending::Import);
        card(cx)
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                h_flex()
                    .flex_wrap()
                    .items_start()
                    .justify_between()
                    .gap_3()
                    .px(px(18.))
                    .pt_4()
                    .pb_3()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(180.))
                            .gap_0p5()
                            .child(card_title(t!("tongue.model_in_use")))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(palette::text_3())
                                    .child(t!("tongue.model_in_use_about")),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .gap_1()
                            .child(
                                Button::new("import-folder")
                                    .ghost()
                                    .small()
                                    .icon(IconName::FolderInput)
                                    .label(t!("tongue.import_folder"))
                                    .tooltip(t!("tongue.import_folder_tooltip"))
                                    .loading(importing)
                                    .disabled(import_blocked)
                                    .on_click(cx.listener(|section, _, _, cx| {
                                        section.import_model(false, cx)
                                    })),
                            )
                            .child(
                                Button::new("import-zip")
                                    .ghost()
                                    .small()
                                    .icon(IconName::FileArchive)
                                    .label(t!("tongue.import_zip"))
                                    .tooltip(t!("tongue.import_zip_tooltip"))
                                    .disabled(import_blocked)
                                    .on_click(cx.listener(|section, _, _, cx| {
                                        section.import_model(true, cx)
                                    })),
                            ),
                    ),
            )
            .children(
                self.qftplus_panel(self.active_model().is_none(), cx)
                    .map(|qftplus| div().px(px(18.)).pb_3().child(qftplus)),
            )
            .children(
                self.transfer_panel()
                    .map(|transfer| div().px(px(18.)).pb_3().child(transfer)),
            )
            .child(v_flex().children(rows))
            .when(override_set, |panel| {
                panel.child(
                    div()
                        .px(px(18.))
                        .pt_3()
                        .child(Notice::new(Tone::Waiting, t!("tongue.set_by_override"))),
                )
            })
            .children(model_message.map(|message| div().px(px(18.)).pt_3().child(message)))
            .child(
                h_flex()
                    .mt_3()
                    .gap_3()
                    .justify_between()
                    .px(px(18.))
                    .py_3()
                    .border_t_1()
                    .border_color(palette::line_soft())
                    .child(hint(t!("tongue.still_unreliable"), cx).flex_1().min_w_0())
                    .child(
                        Button::new("record-again")
                            .small()
                            .label(t!("tongue.record_again"))
                            .on_click(
                                cx.listener(|section, _, _, cx| section.go(Step::Record, cx)),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// Trying the model: which one is in use, and the tongue live.
    fn try_step(&self, wide: bool, cx: &Context<Self>) -> AnyElement {
        let state = self.daemon.read(cx);
        let tongue = crate::camera::tongue_panel(
            crate::camera::tongue_reading(state.status(), &state.rates()),
            state
                .status()
                .and_then(|status| status.output.as_ref())
                .map(|output| output.source),
            cx,
        )
        .child(hint(t!("tongue.try_hint"), cx).pb_1());
        v_flex()
            .gap_4()
            .when(self.just_trained, |step| {
                step.children(self.train_message.clone())
            })
            .child(columns(wide, TRY_SIDE_WIDTH, self.models_panel(cx), tongue))
            .into_any_element()
    }

    /// What recording needs, and how each stands.
    fn readiness(&self, cx: &App) -> [(Cow<'static, str>, summary::Reading); 1] {
        let status = self.daemon.read(cx).status();
        [(
            t!("tongue.mouth_cameras"),
            summary::recording_cameras(status),
        )]
    }

    /// Whether a recording can start now.
    fn can_record(&self, cx: &App) -> bool {
        self.readiness(cx)
            .iter()
            .all(|(_, reading)| reading.tone == Tone::Good)
            && !self.training_busy()
            && self.pending.is_none()
    }

    /// One ruled row per thing recording needs: a dot, what it is, and how
    /// it stands, with a way to fix it when it isn't ready.
    fn readiness_list(
        &self,
        readiness: [(Cow<'static, str>, summary::Reading); 1],
    ) -> impl IntoElement {
        let rows = readiness
            .into_iter()
            .enumerate()
            .map(|(index, (label, reading))| {
                let ready = reading.tone == Tone::Good;
                let fix = reading.fix.filter(|_| !ready);
                v_flex()
                    .gap_0p5()
                    .py(px(9.))
                    .border_t_1()
                    .border_color(palette::line_soft())
                    .child(
                        h_flex()
                            .gap_3()
                            .min_h(px(24.))
                            .text_size(px(13.))
                            .child(StatusDot::new(reading.tone))
                            .child(div().flex_none().text_color(palette::text_3()).child(label))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_medium()
                                    .child(reading.value),
                            )
                            .when_some(fix, |row, fix| {
                                row.child(
                                    fix_button(("fix-readiness", index), fix).ghost().xsmall(),
                                )
                            }),
                    )
                    .when(!ready && !reading.detail.is_empty(), |this| {
                        this.child(
                            div()
                                .pl(px(24.))
                                .text_xs()
                                .text_color(palette::text_3())
                                .child(reading.detail),
                        )
                    })
            });
        v_flex().children(rows)
    }

    /// Recording: what it involves, whether everything's ready, and tips,
    /// beside the recordings made so far and the optional extra ones.
    fn record_step(&self, wide: bool, main_width: f32, cx: &Context<Self>) -> AnyElement {
        let readiness = self.readiness(cx);
        let can_record = self.can_record(cx);
        let enough = self.enough();
        let recorded = self.recorded();
        let outcome = self
            .capture
            .as_ref()
            .filter(|capture| !capture.message.is_empty())
            .map(|capture| {
                let complete = capture.message.starts_with("Recording complete");
                // A face setup is good once it's in use.
                let good = match last_face_setup(capture) {
                    Some(report) => report.in_use,
                    None => complete,
                };
                v_flex()
                    .gap_2()
                    .child(Notice::new(
                        if good { Tone::Good } else { Tone::Waiting },
                        capture.message.clone(),
                    ))
                    .children(last_face_setup(capture).and_then(face_setup_misses))
            });
        let tips = v_flex().children(TIPS.iter().map(|tip| {
            h_flex()
                .gap_2p5()
                .items_start()
                .py(px(9.))
                .border_t_1()
                .border_color(palette::line_soft())
                .text_size(px(13.))
                .text_color(palette::text_2())
                .child(
                    div()
                        .flex_none()
                        .h(px(19.))
                        .flex()
                        .items_center()
                        .text_color(palette::text_3())
                        .child(Icon::new(IconName::Lightbulb).size(px(14.))),
                )
                .child(div().flex_1().min_w_0().child(tip()))
        }));
        // Once there's enough to train, training is the way forward and
        // recording again is the alternative.
        let start = Button::new("record-core")
            .prominent()
            .icon(IconName::Play)
            .label(if recorded {
                t!("tongue.record_basic_again")
            } else {
                t!("tongue.start_recording")
            })
            .when(!enough, |button| button.primary())
            .loading(self.pending == Some(Pending::StartRecording(CaptureMode::Core)))
            .disabled(!can_record)
            .on_click(cx.listener(|section, _, window, cx| {
                window.focus(&section.focus, cx);
                section.start_recording(CaptureMode::Core, cx)
            }));
        let record = step_card(cx)
            .child(heading(
                t!("tongue.record_heading"),
                t!("tongue.record_lead"),
            ))
            // Training starts from the mouth-camera pair and mixes in the
            // examples; downloading them now means they're ready by the time
            // the recording is.
            .when_some(
                self.training_files_missing()
                    .then(|| self.training_files_panel(false, cx))
                    .flatten(),
                |panel, builtin| {
                    panel.child(
                        v_flex()
                            .gap_2()
                            .child(builtin)
                            .child(hint(t!("tongue.download_while_recording"), cx)),
                    )
                },
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(cap(t!("tongue.before_you_start")))
                    .child(self.readiness_list(readiness)),
            )
            .child(v_flex().gap_2().child(cap(t!("tongue.tips"))).child(tips))
            .when(self.training_busy(), |panel| {
                panel.child(Notice::new(Tone::Waiting, t!("tongue.training_running")))
            })
            .children(outcome)
            .children(self.record_error.clone())
            .when(enough, |panel| {
                panel.child(Notice::new(Tone::Good, t!("tongue.recordings_enough")))
            })
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .when(enough, |row| {
                        row.child(
                            Button::new("to-train")
                                .primary()
                                .prominent()
                                .icon(IconName::Sparkles)
                                .label(t!("tongue.continue_to_training"))
                                .on_click(
                                    cx.listener(|section, _, _, cx| section.go(Step::Train, cx)),
                                ),
                        )
                    })
                    .child(start),
            )
            .child(
                v_flex()
                    .mx(px(-22.))
                    .px(px(22.))
                    .pt_4()
                    .border_t_1()
                    .border_color(palette::line_soft())
                    .child(self.voice_controls("read-aloud", cx)),
            );
        let main = self
            .review_card(main_width, cx)
            .unwrap_or_else(|| record.into_any_element());
        columns(wide, SIDE_WIDTH, main, self.side_cards(can_record, cx)).into_any_element()
    }

    /// The recordings and the extra recordings, beside the Record and Train
    /// steps.
    fn side_cards(&self, can_record: bool, cx: &Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_4()
            .child(self.recordings_card(cx))
            .child(self.extras_card(can_record, cx))
    }

    /// Every recording on this PC, newest first, each ticked to train on it.
    fn recordings_card(&self, cx: &Context<Self>) -> AnyElement {
        let now = Local::now();
        let rows: Vec<AnyElement> = self
            .recordings
            .iter()
            .rev()
            .enumerate()
            .map(|(index, recording)| self.recording_row(index, recording, now, cx))
            .collect();
        let empty = rows.is_empty();
        card(cx)
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                h_flex()
                    .justify_between()
                    .gap_3()
                    .px_4()
                    .pt(px(14.))
                    .pb_3()
                    .child(card_title(t!("tongue.your_recordings"))),
            )
            .when(empty, |panel| {
                panel.child(
                    hint(t!("tongue.recordings_appear_here"), cx)
                        .px_4()
                        .pb(px(14.)),
                )
            })
            .children(rows)
            .children(self.list_message.clone().map(|message| {
                div()
                    .p_3()
                    .border_t_1()
                    .border_color(palette::line_soft())
                    .child(message)
            }))
            .into_any_element()
    }

    /// How the face setup stands: the one in use, and what it fitted; none
    /// yet; or that it needs the five cameras on.
    fn face_setup_state(&self, cx: &App) -> Option<(Tone, Cow<'static, str>)> {
        let status = self.daemon.read(cx).status()?;
        if !status.five_cameras {
            return Some((Tone::Waiting, t!("tongue.face_setup_needs_five")));
        }
        let face = status.face_model.as_ref()?;
        if face.enrollment_error.is_some() {
            return Some((Tone::Problem, t!("tongue.face_setup_unreadable")));
        }
        Some(match &face.enrollment {
            Some(id) => {
                let made = created_at(id)
                    .map(|ms| when(ms, Local::now(), true))
                    .unwrap_or_default();
                let fitted = if face.tongue_map {
                    t!("tongue.face_setup_tongue_fitted")
                } else {
                    t!("tongue.face_setup_tongue_not_fitted")
                };
                (
                    Tone::Good,
                    t!("tongue.face_setup_in_use", when = made, tongue = fitted),
                )
            }
            None if face.loaded => (Tone::Waiting, t!("tongue.face_setup_none")),
            None => return None,
        })
    }

    /// The optional recordings, each for one kind of movement.
    fn extras_card(&self, can_record: bool, cx: &Context<Self>) -> AnyElement {
        // The face setup reads every camera.
        let five_cameras = self
            .daemon
            .read(cx)
            .status()
            .is_some_and(|status| status.five_cameras);
        let rows = EXTRAS
            .into_iter()
            .enumerate()
            .map(|(index, (mode, icon, why))| {
                let (name, about) = MODES.iter().find(|(id, _, _)| *id == mode).map_or_else(
                    || (t!("tongue.recording"), Cow::Borrowed("")),
                    |(_, name, about)| (name(), about()),
                );
                let face_setup = mode == CaptureMode::Enrollment;
                let state = face_setup.then(|| self.face_setup_state(cx)).flatten();
                let blocked = face_setup && !five_cameras;
                h_flex()
                    .gap_3()
                    .px_4()
                    .py(px(11.))
                    .border_t_1()
                    .border_color(palette::line_soft())
                    .child(
                        div()
                            .flex_none()
                            .text_color(palette::text_2())
                            .child(Icon::new(icon).size(px(16.))),
                    )
                    .child(
                        // When it helps, on hover: each is worth a read before
                        // recording it.
                        v_flex()
                            .id(("extra", index))
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .tooltip(move |window, cx| {
                                Tooltip::new(SharedString::from(why())).build(window, cx)
                            })
                            .child(div().text_size(px(13.)).child(name))
                            .child(div().text_xs().text_color(palette::text_3()).child(about))
                            .children(state.map(|(tone, text)| {
                                div()
                                    .text_xs()
                                    .text_color(if tone == Tone::Problem {
                                        palette::signal_text()
                                    } else {
                                        palette::text_2()
                                    })
                                    .child(text)
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("record-{}", mode.name())))
                            .small()
                            .label(t!("tongue.record"))
                            .loading(self.pending == Some(Pending::StartRecording(mode)))
                            .disabled(!can_record || blocked)
                            .on_click(cx.listener(move |section, _, window, cx| {
                                window.focus(&section.focus, cx);
                                section.start_recording(mode, cx)
                            })),
                    )
            });
        card(cx)
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                v_flex()
                    .gap_0p5()
                    .px_4()
                    .pt(px(14.))
                    .pb_3()
                    .child(card_title(t!("tongue.extra_recordings")))
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette::text_3())
                            .child(t!("tongue.extra_recordings_about")),
                    ),
            )
            .children(rows)
            .into_any_element()
    }

    /// The mouth cameras while recording, small in the stage's corner, so
    /// the wearer can check their tongue is in view.
    fn camera_view(&self, cx: &Context<Self>) -> AnyElement {
        let live = self
            .daemon
            .read(cx)
            .status()
            .is_some_and(summary::camera_live);
        let image = self.camera.read(cx).image().filter(|_| live);
        let showing = image.is_some();
        let view = div()
            .relative()
            .w_full()
            .h(px(108.))
            .rounded(px(10.))
            .overflow_hidden()
            .bg(black())
            .border_1()
            .border_color(option_line());
        v_flex()
            .flex_none()
            .w(px(216.))
            .gap(px(6.))
            .child(match image {
                Some(image) => view
                    .child(
                        img(image)
                            .size_full()
                            .rounded(px(9.))
                            .object_fit(ObjectFit::Contain),
                    )
                    .child(
                        h_flex()
                            .absolute()
                            .left(px(8.))
                            .top(px(7.))
                            .gap_1p5()
                            .child(div().size(px(5.)).rounded_full().bg(palette::text()))
                            .child(cap(t!("tongue.live")).text_color(palette::text())),
                    ),
                None => view
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .text_color(palette::text_3())
                    .child(Icon::new(IconName::CameraOff).size(px(16.)))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .child(t!("tongue.no_camera_image")),
                    ),
            })
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(palette::text_3())
                    .text_right()
                    .child(if showing {
                        t!("tongue.mouth_cameras")
                    } else {
                        t!("tongue.cameras_stopped_sending")
                    }),
            )
            .into_any_element()
    }

    /// The running recording, over the whole page: where it's up to, the
    /// pose to do and its countdown, the cameras, and controls.
    fn prompt_panel(
        &self,
        capture: &CaptureStatus,
        wide: bool,
        height: f32,
        cx: &Context<Self>,
    ) -> AnyElement {
        let follow = capture.mode == Some(CaptureMode::Follow);
        let step_seconds = capture.step_seconds.max(0.1);
        let elapsed = (step_seconds - capture.seconds_remaining.unwrap_or(step_seconds)).max(0.);
        let (phase, tone) = phase(capture);
        let total = capture.total_steps.unwrap_or(1).max(1);
        let step = capture.step.unwrap_or(1).clamp(1, total);
        let where_up_to = if follow {
            t!("tongue.part_of", step = step, total = total)
        } else {
            t!("tongue.pose_of", step = step, total = total)
        };
        let pad = follow.then(|| {
            let pad = match &capture.path {
                Some(path) => {
                    let seconds = capture.path_elapsed.unwrap_or_default()
                        + if capture.paused {
                            0.
                        } else {
                            self.capture_at.elapsed().as_secs_f32()
                        };
                    let [horizontal, vertical] = path_position(path, seconds.max(0.));
                    // The stops still ahead, so the next move can be anticipated.
                    let marks = path
                        .iter()
                        .filter(|[time, _, _]| *time > seconds)
                        .map(|[_, h, v]| [*h, *v])
                        .collect();
                    DirectionPad::new(280.)
                        .dot(horizontal, vertical, 30.)
                        .marks(marks)
                        .halo((seconds < 0.).then(|| (-seconds).min(1.5) * 30.))
                }
                None => DirectionPad::new(280.).caption(t!("tongue.tongue_in")),
            };
            div().flex().justify_center().child(pad)
        });
        let busy = self.pending.is_some();

        let top =
            v_flex()
                .flex_none()
                .gap_3()
                .child(
                    h_flex()
                        .gap(px(10.))
                        .items_baseline()
                        .child(
                            div()
                                .text_size(px(15.))
                                .font_semibold()
                                .child(mode_name(capture.mode)),
                        )
                        .child(
                            mono(where_up_to)
                                .text_size(px(12.5))
                                .text_color(palette::text_3()),
                        )
                        .child(div().flex_1())
                        .children(time_left(capture).map(|left| {
                            mono(left).text_size(px(12.5)).text_color(palette::text_3())
                        })),
                )
                .child(progress_segments(
                    step,
                    total,
                    (elapsed / step_seconds).clamp(0., 1.),
                ));

        // The last thing said aloud, for when the voice was missed.
        let said = self.last_cue.clone().filter(|_| self.voice).map(|cue| {
            h_flex()
                .gap_2()
                .max_w(px(280.))
                .text_size(px(12.5))
                .text_color(palette::text_3())
                .child(
                    div()
                        .flex_none()
                        .child(Icon::new(IconName::Volume2).size(px(15.))),
                )
                .child(div().flex_none().child(t!("tongue.said")))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(soft_text())
                        .child(format!("\u{201c}{cue}\u{201d}")),
                )
        });
        let cameras = self.camera_view(cx);
        let name_size = if follow { 44. } else { 62. };
        let prompt = v_flex()
            .flex_1()
            .w_full()
            .items_center()
            .justify_center()
            .gap_4()
            .py_4()
            .text_center()
            .child(
                div()
                    .text_size(px(name_size))
                    .line_height(px(name_size + 4.))
                    .font_semibold()
                    .child(capture.pose.clone().unwrap_or_default()),
            )
            .child(
                div()
                    .max_w(px(560.))
                    .text_size(px(20.))
                    .line_height(px(28.))
                    .text_color(palette::text_2())
                    .child(capture.instruction.clone().unwrap_or_default()),
            )
            .children(pad);
        let next = h_flex()
            .mt(px(22.))
            .gap(px(10.))
            .text_size(px(14.))
            .map(|row| match &capture.next_pose {
                Some(next) => row
                    .child(cap(t!("tongue.up_next")))
                    .child(div().text_color(soft_text()).child(next.clone())),
                None => row.child(div().text_color(palette::text_3()).child(if follow {
                    t!("tongue.last_part")
                } else {
                    t!("tongue.last_pose")
                })),
            });
        let stage = v_flex()
            .relative()
            .flex_1()
            .w_full()
            .items_center()
            .px(px(32.))
            .pt(px(30.))
            .pb(px(26.))
            .rounded(px(14.))
            .border_1()
            .border_color(palette::line())
            .bg(palette::rail())
            .overflow_hidden()
            .map(|stage| {
                if wide {
                    stage
                        .children(said.map(|said| said.absolute().left(px(20.)).top(px(20.))))
                        .child(div().absolute().right(px(16.)).top(px(16.)).child(cameras))
                } else {
                    stage.child(
                        h_flex()
                            .w_full()
                            .items_start()
                            .gap_3()
                            .mb_4()
                            .child(div().flex_1().min_w_0().children(said))
                            .child(cameras),
                    )
                }
            })
            .child(phase_chip(&phase, tone))
            .child(prompt)
            .child(PoseTimer {
                settle: capture.settle_seconds,
                step: step_seconds,
                elapsed,
                recording: capture.recording,
            })
            .child(next);

        let controls = h_flex()
            .flex_none()
            .gap(px(10.))
            .flex_wrap()
            .child(
                div()
                    .flex_1()
                    .min_w(px(260.))
                    .child(self.voice_row("read-aloud-recording", cx)),
            )
            .child(
                Button::new("pause-recording")
                    .prominent()
                    .h(px(48.))
                    .px(px(18.))
                    .icon(if capture.paused {
                        IconName::Play
                    } else {
                        IconName::Pause
                    })
                    .label(if capture.paused {
                        t!("tongue.resume")
                    } else {
                        t!("tongue.pause")
                    })
                    .children(key_hint("p", false))
                    .loading(self.pending == Some(Pending::Capture(CaptureCommand::Pause)))
                    .disabled(busy)
                    .on_click(cx.listener(|section, _, _, cx| {
                        section.capture_command(CaptureCommand::Pause, cx)
                    })),
            )
            .child(
                Button::new("skip-pose")
                    .prominent()
                    .h(px(48.))
                    .px(px(18.))
                    .icon(IconName::SkipForward)
                    .label(if follow {
                        t!("tongue.skip_part")
                    } else {
                        t!("tongue.skip_pose")
                    })
                    .children(key_hint("s", false))
                    .loading(self.pending == Some(Pending::Capture(CaptureCommand::Skip)))
                    .disabled(busy || capture.skipped)
                    .on_click(cx.listener(|section, _, _, cx| {
                        section.capture_command(CaptureCommand::Skip, cx)
                    })),
            )
            .child(
                Button::new("stop-recording")
                    .danger()
                    .outline()
                    .prominent()
                    .h(px(48.))
                    .px(px(18.))
                    .icon(IconName::CircleStop)
                    .label(if self.confirm_stop {
                        t!("tongue.stop_now")
                    } else {
                        t!("tongue.stop")
                    })
                    .children(key_hint("escape", true))
                    .loading(self.pending == Some(Pending::Capture(CaptureCommand::Stop)))
                    .disabled(busy)
                    .on_click(cx.listener(|section, _, _, cx| section.stop_pressed(cx))),
            )
            .when(self.confirm_stop, |row| {
                row.child(
                    Button::new("keep-recording")
                        .ghost()
                        .prominent()
                        .h(px(48.))
                        .label(t!("tongue.keep_recording"))
                        .on_click(cx.listener(|section, _, _, cx| section.keep_recording(cx))),
                )
            });

        let view = v_flex()
            .min_h(px(height))
            .gap_4()
            .child(top)
            .child(stage)
            .when(self.confirm_stop, |view| {
                view.child(Notice::new(Tone::Waiting, t!("tongue.stop_confirm")))
            })
            .children(self.voice_notice())
            .children(self.record_error.clone())
            .child(controls);
        // The recording's keys work while focus is anywhere in it.
        div()
            .track_focus(&self.focus)
            .key_context(RECORDING_KEYS)
            .on_action(cx.listener(|section, _: &PauseRecording, _, cx| {
                section.capture_command(CaptureCommand::Pause, cx)
            }))
            .on_action(cx.listener(|section, _: &SkipPose, _, cx| {
                section.capture_command(CaptureCommand::Skip, cx)
            }))
            .on_action(cx.listener(|section, _: &StopRecording, _, cx| section.stop_pressed(cx)))
            .child(view)
            .into_any_element()
    }

    /// Training: what it does, whether the recordings cover enough, its
    /// progress, and the options most people leave alone folded away, beside
    /// the recordings it trains on.
    fn train_step(&self, wide: bool, main_width: f32, cx: &Context<Self>) -> AnyElement {
        let busy = self.training_busy();
        let ticked = self.ticked();
        let lacking = missing(&ticked);
        // A short recording of just what's missing, rather than every basic pose.
        let record_missing =
            (!busy && self.recorded() && !ticked.is_empty() && !lacking.is_empty()).then(|| {
                let lacking = lacking.clone();
                let seconds = lacking.len() * 8;
                Button::new("record-missing")
                    .primary()
                    .prominent()
                    .icon(IconName::Radio)
                    .label(t!("tongue.record_just_those", seconds = seconds))
                    .disabled(self.capture_active() || self.pending.is_some())
                    .on_click(cx.listener(move |section, _, window, cx| {
                        section.record_missing(&lacking, window, cx)
                    }))
            });
        let recorded = self.recorded();
        let files_missing = self.training_files_missing();
        let can_train = !busy
            && !self.capture_active()
            && self.pending.is_none()
            && !files_missing
            && !ticked.is_empty()
            && lacking.is_empty();
        let ready = recorded && !ticked.is_empty() && lacking.is_empty();
        // Training starts from the mouth-camera pair, so that comes first;
        // its download is the next step once it's all that's missing.
        let base = files_missing
            .then(|| self.training_files_panel(ready, cx))
            .flatten();
        let covered = (recorded && !ticked.is_empty() && !busy).then(|| {
            let cells = coverage(&ticked).into_iter().map(|(name, have, needed)| {
                v_flex()
                    .min_w_0()
                    .gap_2()
                    .px(px(14.))
                    .py_3()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(cell_line())
                    .bg(palette::inset())
                    .child(
                        h_flex()
                            .justify_between()
                            .gap_2()
                            .text_size(px(12.5))
                            .child(div().truncate().child(need_label(name)))
                            .child(
                                mono(format!("{have}/{needed}"))
                                    .flex_none()
                                    .text_color(palette::text_3()),
                            ),
                    )
                    // Full and white once there's enough.
                    .child(Meter::new(have as f32 / needed as f32, palette::text()))
            });
            // Cheek puffs are optional, so they get a line rather than a cell.
            let cheeks = (!cheeks_covered(&ticked)).then(|| {
                h_flex()
                    .flex_wrap()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette::text_3())
                            .child(t!("tongue.no_cheek_puffs")),
                    )
                    .child(
                        Button::new("record-cheeks")
                            .small()
                            .label(t!(
                                "tongue.record_cheek_poses",
                                seconds = CHEEK_POSES.len() * 8
                            ))
                            .disabled(self.capture_active() || self.pending.is_some())
                            .on_click(cx.listener(|section, _, window, cx| {
                                section.record_cheeks(window, cx)
                            })),
                    )
            });
            v_flex()
                .gap(px(10.))
                .child(cap(t!("tongue.positions_covered")))
                .child(
                    div()
                        .grid()
                        .grid_cols(if main_width >= 420. { 3 } else { 2 })
                        .gap_2()
                        .children(cells),
                )
                .children(cheeks)
        });
        let try_it = self.just_trained.then(|| {
            Button::new("try-model")
                .prominent()
                .label(t!("tongue.test_it"))
                .on_click(cx.listener(|section, _, _, cx| section.go(Step::Try, cx)))
        });
        // Where training stands before it starts: ready, with the button that
        // starts it, or what's still missing and the way to get it.
        let standing = (!busy).then(|| {
            let (lead, actions) = if ready {
                let frames = ticked.iter().map(|recording| recording.frames).sum();
                let lead = v_flex()
                    .gap(px(3.))
                    .child(
                        h_flex()
                            .gap(px(7.))
                            .text_size(px(13.5))
                            .font_medium()
                            .child(Icon::new(IconName::Check).size(px(14.)))
                            .child(t!(
                                "tongue.ready",
                                recordings = recording_count(ticked.len() as u64),
                                frames = frame_count(frames)
                            )),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette::text_3())
                            .child(estimate(self.device)),
                    );
                let actions = h_flex()
                    .gap_2()
                    .child(
                        Button::new("train")
                            .prominent()
                            .icon(IconName::Sparkles)
                            .label(t!("tongue.train"))
                            // The training files' download comes first.
                            .when(!files_missing, |button| button.primary())
                            .loading(self.pending == Some(Pending::Train))
                            .disabled(!can_train)
                            .on_click(cx.listener(|section, _, _, cx| section.start_training(cx))),
                    )
                    .children(try_it);
                (lead, actions)
            } else {
                let text = if !recorded {
                    t!("tongue.record_basic_first")
                } else if ticked.is_empty() {
                    t!("tongue.all_left_out")
                } else {
                    t!("tongue.needs_more_frames", positions = positions(&lacking))
                };
                let lead = h_flex()
                    .gap_2()
                    .items_start()
                    .text_size(px(13.5))
                    .font_medium()
                    .child(
                        div()
                            .flex_none()
                            .h(px(20.))
                            .flex()
                            .items_center()
                            .child(StatusDot::new(Tone::Waiting)),
                    )
                    .child(div().flex_1().min_w_0().child(text));
                let has_missing = record_missing.is_some();
                let actions = h_flex()
                    .gap_2()
                    .flex_wrap()
                    .children(record_missing)
                    .when(!recorded || !lacking.is_empty(), |row| {
                        row.child(
                            Button::new("record-more")
                                .prominent()
                                .icon(IconName::Radio)
                                .label(if recorded {
                                    t!("tongue.record_more")
                                } else {
                                    t!("tongue.go_to_recording")
                                })
                                .when(!has_missing, |button| button.primary())
                                .on_click(
                                    cx.listener(|section, _, _, cx| section.go(Step::Record, cx)),
                                ),
                        )
                    })
                    .children(try_it);
                (lead, actions)
            };
            h_flex()
                .flex_wrap()
                .gap_3()
                .px_4()
                .py(px(14.))
                .rounded(px(11.))
                .bg(palette::raised())
                .border_1()
                .border_color(palette::line_strong())
                .child(div().flex_1().min_w(px(200.)).child(lead))
                .child(actions)
        });
        let progress = self
            .training
            .as_ref()
            .filter(|training| training.busy)
            .map(|training| {
                let progress = training.progress.clone().unwrap_or_default();
                let fraction = progress.fraction.unwrap_or(0.).clamp(0., 1.);
                let left = training_time_left(&progress);
                let message = if progress.message.is_empty() {
                    t!("tongue.starting").to_string()
                } else {
                    progress.message
                };
                let on_cpu = progress
                    .device
                    .as_deref()
                    .is_some_and(|device| device.to_lowercase().contains("cpu"));
                let device = progress.device.clone();
                let cancel = if self.confirm_cancel {
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(div().text_sm().child(t!("tongue.cancel_training_question")))
                        .child(
                            Button::new("confirm-cancel")
                                .danger()
                                .outline()
                                .small()
                                .label(t!("tongue.cancel_training"))
                                .loading(self.pending == Some(Pending::Cancel))
                                .on_click(
                                    cx.listener(|section, _, _, cx| section.cancel_training(cx)),
                                ),
                        )
                        .child(
                            Button::new("keep-training")
                                .ghost()
                                .small()
                                .label(t!("tongue.keep_training"))
                                .on_click(cx.listener(|section, _, _, cx| {
                                    section.confirm_cancel = false;
                                    cx.notify();
                                })),
                        )
                } else {
                    h_flex().child(
                        Button::new("cancel-training")
                            .regular()
                            .label(t!("tongue.cancel_training"))
                            .on_click(cx.listener(|section, _, _, cx| {
                                section.confirm_cancel = true;
                                cx.notify();
                            })),
                    )
                };
                v_flex()
                    .gap_2()
                    .px_4()
                    .py(px(14.))
                    .rounded(px(11.))
                    .bg(palette::raised())
                    .border_1()
                    .border_color(palette::line_strong())
                    .child(
                        h_flex()
                            .justify_between()
                            .gap_2()
                            .text_size(px(13.5))
                            .child(div().min_w_0().child(format!("{message}{left}")))
                            .child(
                                mono(format!("{:.0}%", fraction * 100.))
                                    .flex_none()
                                    .font_medium(),
                            ),
                    )
                    .child(Meter::new(fraction, palette::text()))
                    .children(
                        device.map(|device| hint(t!("tongue.training_on", device = device), cx)),
                    )
                    .when(on_cpu && self.device == Device::Automatic, |this| {
                        this.child(Notice::new(Tone::Waiting, t!("tongue.no_gpu_found")))
                    })
                    .child(hint(t!("tongue.you_can_leave"), cx))
                    .child(div().pt_1().child(cancel))
            });
        let devices = h_flex()
            .gap_0p5()
            .p(px(3.))
            .rounded(px(9.))
            .bg(palette::sunken())
            .border_1()
            .border_color(palette::line())
            .children(Device::ALL.into_iter().map(|device| {
                Button::new(("device", device as usize))
                    .small()
                    .ghost()
                    .selected(self.device == device)
                    .label(device.label())
                    .on_click(cx.listener(move |section, _, _, cx| {
                        section.device = device;
                        cx.notify();
                    }))
            }));
        let architectures = h_flex()
            .gap_0p5()
            .p(px(3.))
            .rounded(px(9.))
            .bg(palette::sunken())
            .border_1()
            .border_color(palette::line())
            .children(
                [
                    (
                        TrainerArchitecture::StereoPair,
                        t!("tongue.architecture_pair"),
                    ),
                    (
                        TrainerArchitecture::UniversalFace,
                        t!("tongue.architecture_face"),
                    ),
                ]
                .into_iter()
                .enumerate()
                .map(|(index, (architecture, label))| {
                    Button::new(("architecture", index))
                        .small()
                        .ghost()
                        .selected(self.architecture == architecture)
                        .label(label)
                        .on_click(cx.listener(move |section, _, _, cx| {
                            section.architecture = architecture;
                            cx.notify();
                        }))
                }),
            );
        // Folded away along the card's foot, for what most people leave alone.
        let advanced = v_flex()
            .mx(px(-22.))
            .border_t_1()
            .border_color(palette::line_soft())
            .child(
                h_flex()
                    .id("training-options")
                    .gap(px(10.))
                    .px(px(22.))
                    .py(px(14.))
                    .cursor_pointer()
                    .hover(|style| style.bg(palette::inset()))
                    .on_click(cx.listener(|section, _, _, cx| {
                        section.show_advanced = !section.show_advanced;
                        cx.notify();
                    }))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(soft_text())
                                    .child(t!("tongue.advanced_options")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(palette::text_3())
                                    .child(t!("tongue.advanced_options_about")),
                            ),
                    )
                    .child(
                        div().flex_none().text_color(palette::text_3()).child(
                            Icon::new(if self.show_advanced {
                                IconName::ChevronUp
                            } else {
                                IconName::ChevronDown
                            })
                            .size(px(14.)),
                        ),
                    ),
            )
            .when(self.show_advanced, |section| {
                section.child(
                    v_flex()
                        .gap_4()
                        .px(px(22.))
                        .pt_1()
                        .pb(px(20.))
                        .child(option_row(
                            t!("tongue.model_name"),
                            None,
                            Input::new(&self.name).small(),
                            cx,
                        ))
                        .child(option_row(
                            t!("tongue.device"),
                            Some(t!("tongue.device_about")),
                            devices,
                            cx,
                        ))
                        .child(option_row(
                            t!("tongue.architecture"),
                            Some(t!("tongue.architecture_about")),
                            architectures,
                            cx,
                        ))
                        .child(option_row(
                            t!("tongue.training_passes"),
                            Some(t!("tongue.training_passes_about")),
                            div().w(px(80.)).child(Input::new(&self.epochs).small()),
                            cx,
                        )),
                )
            });
        let train = card(cx)
            .flex()
            .flex_col()
            .gap_4()
            .px(px(22.))
            .pt(px(20.))
            .overflow_hidden()
            .child(heading(t!("tongue.train_heading"), t!("tongue.train_lead")))
            .children(base)
            .children(covered)
            .children(standing)
            .children(progress)
            .children(self.train_message.clone())
            .children(self.failure_actions(busy, cx))
            .child(advanced);
        let main = self
            .review_card(main_width, cx)
            .unwrap_or_else(|| train.into_any_element());
        columns(
            wide,
            SIDE_WIDTH,
            main,
            self.side_cards(self.can_record(cx), cx),
        )
        .into_any_element()
    }

    fn recording_row(
        &self,
        index: usize,
        recording: &Recording,
        now: DateTime<Local>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let id = recording.id.clone();
        let readable = recording.error.is_none();
        let ticked = !self.unticked.contains(&recording.id);
        let tag = if !readable {
            t!("tongue.tag_unreadable")
        } else if !ticked {
            t!("tongue.tag_left_out")
        } else if recording.basic_ready {
            t!("tongue.tag_complete")
        } else if recording.mode == Some(CaptureMode::Core) {
            t!("tongue.tag_incomplete")
        } else {
            t!("tongue.tag_extra")
        };
        let title = recording_title(recording, now);
        let suspects = recording
            .poses
            .iter()
            .filter(|pose| pose.suspect && !pose.excluded)
            .count() as u64;
        let confirming = self.confirm_delete.as_ref() == Some(&recording.id);
        // Under the title: what deleting does while it's asked, why it
        // can't be read, poses worth a look, or its frames and how it stands.
        let detail = if confirming {
            div()
                .text_xs()
                .text_color(palette::text_3())
                .child(t!("tongue.delete_recording_detail"))
                .into_any_element()
        } else if let Some(error) = recording.error.clone() {
            warning_line(error).into_any_element()
        } else if suspects > 0 {
            warning_line(if suspects == 1 {
                t!("tongue.suspect_one", count = grouped(suspects))
            } else {
                t!("tongue.suspect_other", count = grouped(suspects))
            })
            .into_any_element()
        } else {
            div()
                .text_xs()
                .text_color(palette::text_3())
                .child(t!(
                    "tongue.frames_tag",
                    frames = frame_count(recording.frames),
                    tag = tag
                ))
                .into_any_element()
        };
        let reviewing = self.reviewing.as_ref() == Some(&recording.id);
        let locked = self.training_busy() || self.capture_active() || self.pending.is_some();
        let actions = if confirming {
            h_flex()
                .gap_1()
                .child(
                    Button::new(("confirm-delete", index))
                        .danger()
                        .outline()
                        .small()
                        .label(t!("tongue.delete"))
                        .disabled(locked)
                        .on_click(cx.listener({
                            let id = id.clone();
                            move |section, _, _, cx| section.delete(id.clone(), cx)
                        })),
                )
                .child(
                    Button::new(("keep", index))
                        .ghost()
                        .small()
                        .label(t!("tongue.keep"))
                        .on_click(cx.listener(|section, _, _, cx| {
                            section.confirm_delete = None;
                            cx.notify();
                        })),
                )
        } else {
            h_flex()
                .gap_0p5()
                .when(readable, |row| {
                    row.child(
                        Button::new(("review", index))
                            // Poses worth a look make reviewing the thing to do.
                            .when(suspects == 0 || reviewing, |button| button.ghost())
                            .small()
                            .label(if reviewing {
                                t!("tongue.hide")
                            } else {
                                t!("tongue.review")
                            })
                            .tooltip(t!("tongue.review_tooltip"))
                            .on_click(cx.listener({
                                let id = id.clone();
                                move |section, _, _, cx| section.toggle_review(id.clone(), cx)
                            })),
                    )
                })
                .child(
                    Button::new(("delete", index))
                        .ghost()
                        .small()
                        .icon(IconName::Trash)
                        .tooltip(t!("tongue.delete_recording"))
                        .loading(self.pending == Some(Pending::Delete(id.clone())))
                        .disabled(locked)
                        .on_click(cx.listener({
                            let id = id.clone();
                            move |section, _, _, cx| {
                                section.confirm_delete = Some(id.clone());
                                cx.notify();
                            }
                        })),
                )
        };
        h_flex()
            .gap_3()
            .px_4()
            .py(px(11.))
            .border_t_1()
            .border_color(palette::line_soft())
            .when(reviewing, |row| row.bg(palette::raised()))
            // Choosing recordings for training, beside each one.
            .child(if readable {
                Checkbox::new(("use-recording", index))
                    .tooltip(t!("tongue.use_for_training"))
                    .checked(ticked)
                    .disabled(self.training_busy())
                    .on_click(cx.listener(move |section, checked: &bool, _, cx| {
                        section.tick(id.clone(), *checked, cx)
                    }))
                    .into_any_element()
            } else {
                div()
                    .flex_none()
                    .w(px(16.))
                    .child(StatusDot::new(Tone::Problem))
                    .into_any_element()
            })
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        div()
                            .text_size(px(13.))
                            .truncate()
                            .text_color(if ticked && readable {
                                palette::text()
                            } else {
                                palette::text_2()
                            })
                            .child(title),
                    )
                    .child(detail),
            )
            .child(actions.flex_none())
            .into_any_element()
    }

    /// The recording being reviewed, in place of the step's main card.
    fn review_card(&self, width: f32, cx: &Context<Self>) -> Option<AnyElement> {
        let recording = self
            .reviewing
            .as_ref()
            .and_then(|id| self.recordings.iter().find(|r| r.id == *id))?;
        let id = recording.id.clone();
        Some(
            step_card(cx)
                .child(
                    h_flex()
                        .items_start()
                        .justify_between()
                        .gap_4()
                        .child(
                            v_flex()
                                .min_w_0()
                                .gap_1p5()
                                .child(cap(t!("tongue.review")))
                                .child(heading(
                                    recording_title(recording, Local::now()),
                                    t!("tongue.review_lead"),
                                )),
                        )
                        .child(
                            Button::new("close-review")
                                .small()
                                .label(t!("tongue.done"))
                                .on_click(cx.listener(move |section, _, _, cx| {
                                    section.toggle_review(id.clone(), cx)
                                })),
                        ),
                )
                .child(self.review(recording, if width >= 520. { 3 } else { 2 }, cx))
                .into_any_element(),
        )
    }

    /// Each pose of a recording, to leave out any that went wrong.
    fn review(&self, recording: &Recording, columns: u16, cx: &Context<Self>) -> AnyElement {
        let names = numbered(recording.poses.iter().map(|pose| pose.name.as_str()));
        let locked = self.training_busy() || self.capture_active();
        let tiles = recording.poses.iter().zip(names).map(|(pose, name)| {
            let choice = self.review_choice.get(&pose.step).copied().unwrap_or(1);
            let image = self
                .frames
                .get(&(recording.id.clone(), pose.indices[choice]))
                .cloned();
            let (id, step) = (recording.id.clone(), pose.step);
            // The headset's own tracking disagreed with the prompt.
            let flagged = pose.suspect && !pose.excluded;
            let label = if pose.skipped {
                t!("tongue.pose_skipped", name = name).into_owned()
            } else if flagged {
                t!("tongue.pose_check", name = name).into_owned()
            } else {
                name
            };
            v_flex()
                .min_w_0()
                .rounded(px(10.))
                .border_1()
                .border_color(if flagged {
                    palette::signal_line()
                } else {
                    cell_line()
                })
                .bg(palette::inset())
                .overflow_hidden()
                .child(
                    div()
                        .w_full()
                        .aspect_ratio(2.)
                        .rounded_t(px(9.))
                        .overflow_hidden()
                        .bg(black())
                        .when(pose.excluded, |frame| frame.opacity(0.4))
                        .when_some(image, |frame, image| {
                            frame.child(
                                img(image)
                                    .size_full()
                                    .rounded_t(px(9.))
                                    .object_fit(ObjectFit::Contain),
                            )
                        }),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .p(px(10.))
                        .child(
                            Checkbox::new(("include-pose", pose.step as usize))
                                .checked(!pose.excluded)
                                .disabled(pose.skipped || locked)
                                .label(label)
                                .on_click(cx.listener(move |section, checked: &bool, _, cx| {
                                    section.include_pose(id.clone(), step, *checked, cx)
                                })),
                        )
                        .child(
                            h_flex().gap_0p5().children(
                                [
                                    t!("tongue.frame_start"),
                                    t!("tongue.frame_middle"),
                                    t!("tongue.frame_end"),
                                ]
                                .into_iter()
                                .enumerate()
                                .map(|(which, text)| {
                                    Button::new(("frame", pose.step as usize * 3 + which))
                                        .ghost()
                                        .xsmall()
                                        .selected(choice == which)
                                        .label(text)
                                        .on_click(cx.listener(move |section, _, _, cx| {
                                            section.choose_frame(step, which, cx)
                                        }))
                                }),
                            ),
                        ),
                )
        });
        div()
            .grid()
            .grid_cols(columns)
            .gap_3()
            .children(tiles)
            .into_any_element()
    }
}

impl Render for TongueTraining {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header =
            PageHeader::new(t!("tongue.page_title")).description(t!("tongue.page_description"));
        if !self.online(cx) {
            return v_flex()
                .gap(px(COLUMN_GAP))
                .child(header)
                .child(card(cx).h(px(320.)).child(
                    EmptyState::new(IconName::Unplug, t!("tongue.not_running"), "").action(Some(
                        StartVrft::new(self.launcher.clone()).into_any_element(),
                    )),
                ));
        }
        if let Some(reason) = self.unavailable.clone() {
            return v_flex().gap(px(COLUMN_GAP)).child(header).child(reason);
        }
        let available = vrft_gui_core::content_width(window);
        let wide = available >= TWO_COLUMN_WIDTH;
        // The width the main card of a two-column step gets.
        let main_width = if wide {
            available - SIDE_WIDTH - COLUMN_GAP
        } else {
            available
        };
        // A recording takes the page, so nothing distracts from its prompts.
        if let Some(capture) = self.capture.as_ref().filter(|capture| capture.active) {
            // Move the follow-the-dot dot smoothly between polls.
            if !capture.paused && capture.path.is_some() {
                window.request_animation_frame();
            }
            let height =
                (window.viewport_size().height.as_f32() - PAGE_CHROME).max(MIN_RECORDING_HEIGHT);
            return v_flex().child(self.prompt_panel(capture, wide, height, cx));
        }
        // The model in use is marked on the Try step too; the header says
        // which it is, or that training runs.
        let header = header.trailing(self.header_pill(cx));
        if self.models.is_none() {
            return v_flex()
                .gap(px(COLUMN_GAP))
                .child(header)
                .child(hint(t!("tongue.loading"), cx));
        }
        let step = self.chosen_step.unwrap_or_else(|| self.guided_step());
        let content = match step {
            Step::Record => self.record_step(wide, main_width, cx),
            Step::Train => self.train_step(wide, main_width, cx),
            Step::Try => self.try_step(wide, cx),
        };
        v_flex()
            .gap(px(COLUMN_GAP))
            .child(header)
            .child(self.tabs(step, cx))
            .child(content)
    }
}

/// One step of the Training page's step bar: a numbered mark (ticked once
/// done), its name, and training's progress on Train while it runs.
fn step_tab(
    index: usize,
    step: Step,
    selected: bool,
    done: bool,
    percent: Option<f32>,
) -> gpui_kit::Stateful<Div> {
    let mark = div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(24.))
        .rounded_full();
    let mark = if done {
        mark.bg(palette::text())
            .text_color(palette::page())
            .child(Icon::new(IconName::Check).size(px(14.)))
    } else {
        mark.border_1()
            .border_color(if selected {
                palette::text()
            } else {
                palette::line_focus()
            })
            .font_family(MONO_FONT)
            .text_size(px(12.))
            .text_color(if selected {
                palette::text()
            } else {
                palette::text_3()
            })
            .child(format!("{}", index + 1))
    };
    h_flex()
        .id(SharedString::from(format!("training-tab-{index}")))
        .flex_1()
        .min_w_0()
        .h(px(52.))
        .gap_3()
        .px_4()
        .rounded(px(10.))
        .border_1()
        .cursor_pointer()
        .map(|tab| {
            if selected {
                tab.bg(palette::raised())
                    .border_color(palette::line_focus())
            } else {
                tab.bg(palette::surface())
                    .border_color(palette::line())
                    .hover(|style| {
                        style
                            .border_color(palette::line_strong())
                            .bg(palette::inset())
                    })
            }
        })
        .child(mark)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(15.))
                .font_semibold()
                .text_color(if selected {
                    palette::text()
                } else {
                    palette::text_2()
                })
                .child(step.tab()),
        )
        .children(percent.map(|percent| {
            div()
                .flex_none()
                .font_family(MONO_FONT)
                .text_size(px(12.))
                .text_color(palette::text_2())
                .child(format!("{percent:.0}%"))
        }))
}

/// A setting: its name, the control, and optionally a line on what it does.
fn option_row(
    label: impl Into<SharedString>,
    about: Option<Cow<'static, str>>,
    control: impl IntoElement,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .gap_3()
        .items_start()
        .child(
            div()
                .w(px(140.))
                .flex_none()
                .min_h(px(28.))
                .flex()
                .items_center()
                .text_size(px(13.))
                .text_color(palette::text_3())
                .child(label.into()),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(control)
                .children(about.map(|about| hint(about, cx))),
        )
}

/// A step's main card, with room around its content.
fn step_card(cx: &App) -> Div {
    card(cx)
        .flex()
        .flex_col()
        .gap_4()
        .px(px(22.))
        .pt(px(20.))
        .pb(px(20.))
        .overflow_hidden()
}

/// A main card's title and the paragraph introducing it.
fn heading(title: impl Into<SharedString>, lead: impl Into<SharedString>) -> Div {
    v_flex()
        .min_w_0()
        .gap_1p5()
        .child(
            div()
                .text_size(px(17.))
                .font_semibold()
                .text_color(palette::text())
                .child(title.into()),
        )
        .child(
            div()
                .max_w(px(520.))
                .text_size(px(13.))
                .line_height(px(19.))
                .text_color(palette::text_2())
                .child(lead.into()),
        )
}

/// A side card's title.
fn card_title(title: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(14.))
        .font_semibold()
        .text_color(palette::text())
        .child(title.into())
}

/// A main column and a side column of `side` pixels, side by side when
/// there's room, else one under the other.
fn columns(wide: bool, side: f32, main: impl IntoElement, aside: impl IntoElement) -> Div {
    div()
        .flex()
        .gap(px(COLUMN_GAP))
        .map(|this| {
            if wide {
                this.flex_row().items_start()
            } else {
                this.flex_col()
            }
        })
        .child(div().flex_1().min_w_0().child(main))
        .child(
            div()
                .flex_none()
                .when(wide, |this| this.w(px(side)))
                .child(aside),
        )
}

/// A line that needs a look: a small orange triangle and the words.
fn warning_line(text: impl Into<SharedString>) -> impl IntoElement {
    h_flex()
        .min_w_0()
        .gap_1p5()
        .text_xs()
        .text_color(palette::signal_text())
        .child(
            div()
                .flex_none()
                .text_color(palette::signal())
                .child(Icon::new(IconName::TriangleAlert).size(px(11.))),
        )
        .child(div().min_w_0().truncate().child(text.into()))
}

/// The key that does what a recording button does, on the button.
fn key_hint(key: &str, danger: bool) -> Option<Kbd> {
    let stroke = Keystroke::parse(key).ok()?;
    Some(
        Kbd::new(stroke)
            .flex()
            .items_center()
            .justify_center()
            .min_w(px(24.))
            .h(px(24.))
            .px_1p5()
            .rounded(px(5.))
            .bg(palette::rail())
            .border_1()
            .border_b_2()
            .border_color(if danger {
                palette::signal_line()
            } else {
                key_line()
            })
            .font_family(MONO_FONT)
            .text_size(px(11.))
            .text_color(if danger {
                palette::signal_text()
            } else {
                soft_text()
            }),
    )
}

/// The running pose's phase as a white chip: a solid mark while recording,
/// an arc while getting ready or waiting, a ring while paused or skipped.
fn phase_chip(phase: &str, tone: Tone) -> impl IntoElement {
    let mark = div().flex_none().size(px(8.)).rounded_full();
    h_flex()
        .flex_none()
        .gap(px(9.))
        .h(px(32.))
        .px(px(14.))
        .rounded_full()
        .bg(palette::text())
        .text_color(palette::page())
        .font_family(MONO_FONT)
        .text_size(px(12.))
        .font_semibold()
        .child(match tone {
            Tone::Good => mark.bg(palette::page()).into_any_element(),
            Tone::Waiting | Tone::Problem => div()
                .flex_none()
                .child(Icon::new(IconName::LoaderCircle).size(px(11.)))
                .into_any_element(),
            Tone::Off => mark
                .border_1()
                .border_color(palette::page())
                .into_any_element(),
        })
        .child(phase.to_uppercase())
}

/// One segment per pose: white once done, the current one filling as it
/// runs. Many poses read better as one bar.
fn progress_segments(step: usize, total: usize, current: f32) -> AnyElement {
    if total > MAX_SEGMENTS {
        let done = ((step - 1) as f32 + current) / total as f32;
        return Meter::new(done, palette::text()).into_any_element();
    }
    h_flex()
        .gap_1()
        .children((1..=total).map(|index| {
            let segment = div().flex_1().h(px(4.)).rounded(px(2.));
            if index < step {
                segment.bg(palette::text())
            } else if index == step {
                segment.bg(hover_line()).overflow_hidden().child(
                    div()
                        .h_full()
                        .w(relative(current))
                        .rounded(px(2.))
                        .bg(palette::text()),
                )
            } else {
                segment.bg(option_line())
            }
        }))
        .into_any_element()
}

/// Roughly how long the running recording has left, from the poses to come.
fn time_left(capture: &CaptureStatus) -> Option<String> {
    let step = capture.step?;
    let total = capture.total_steps?;
    let current = capture.seconds_remaining?;
    if capture.step_seconds <= 0. {
        return None;
    }
    let seconds = current + total.saturating_sub(step) as f32 * capture.step_seconds;
    if !seconds.is_finite() {
        return None;
    }
    Some(if seconds < 60. {
        t!("tongue.under_a_minute_left").into()
    } else {
        t!("tongue.minutes_left", minutes = (seconds / 60.).round()).into()
    })
}

/// How long training takes on the chosen device.
fn estimate(device: Device) -> Cow<'static, str> {
    match device {
        Device::Cpu => t!("tongue.estimate_cpu"),
        Device::Automatic | Device::Gpu => t!("tongue.estimate_gpu"),
    }
}

/// Labels beside controls: between the text and its secondary grey.
fn soft_text() -> Hsla {
    rgb(0xd4d4d4).into()
}

/// The edge of a card that isn't chosen when hovered, and grey marks.
fn hover_line() -> Hsla {
    rgb(0x3a3a41).into()
}

/// A track still to fill, and the edge of a small inset.
fn option_line() -> Hsla {
    rgb(0x26262b).into()
}

/// The edge of a cell inside a card.
fn cell_line() -> Hsla {
    rgb(0x1f1f23).into()
}

/// The part of a pose's timeline already done.
fn done_line() -> Hsla {
    rgb(0x4a4a52).into()
}

fn key_line() -> Hsla {
    rgb(0x34343a).into()
}

/// The two parts of a pose, getting ready then recording, how far along it
/// is, and how long each part has.
#[derive(IntoElement)]
struct PoseTimer {
    /// Seconds spent getting ready, of the pose's `step`.
    settle: f32,
    step: f32,
    elapsed: f32,
    /// Whether the pose's frames are being saved.
    recording: bool,
}

impl RenderOnce for PoseTimer {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let step = self.step.max(0.1);
        let settle = self.settle.clamp(0., step);
        let hold = (step - settle).max(0.1);
        let elapsed = self.elapsed.clamp(0., step);
        let settling = !self.recording;
        let ready_label = if settling {
            t!(
                "tongue.get_ready_left",
                seconds = ((settle - elapsed).ceil() as u32).max(1)
            )
        } else {
            t!("tongue.get_ready", seconds = settle.round() as u32)
        };
        let hold_label = if settling {
            t!("tongue.recording_for", seconds = hold.round() as u32)
        } else {
            t!(
                "tongue.recording_left",
                seconds = ((step - elapsed).ceil() as u32).max(1)
            )
        };
        let fill = |fraction: f32| {
            div()
                .h_full()
                .w(relative(fraction.clamp(0., 1.)))
                .rounded(px(4.))
                .bg(palette::text())
        };
        v_flex()
            .w_full()
            .max_w(px(560.))
            .gap_2()
            .child(
                h_flex()
                    .w_full()
                    .h(px(8.))
                    .gap_1()
                    .when(settle > 0., |bar| {
                        bar.child(
                            div()
                                .flex_none()
                                .h_full()
                                .w(relative(settle / step))
                                .rounded(px(4.))
                                .overflow_hidden()
                                .map(|part| {
                                    if settling {
                                        part.bg(option_line()).child(fill(elapsed / settle))
                                    } else {
                                        part.bg(done_line())
                                    }
                                }),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .rounded(px(4.))
                            .overflow_hidden()
                            .bg(option_line())
                            .when(!settling, |part| {
                                part.child(fill((elapsed - settle) / hold))
                            }),
                    ),
            )
            .child(
                h_flex()
                    .justify_between()
                    .gap_4()
                    .font_family(MONO_FONT)
                    .text_size(px(12.))
                    .text_color(palette::text_3())
                    .when(settle > 0., |labels| {
                        labels.child(
                            div()
                                .when(settling, |label| label.text_color(palette::text()))
                                .child(ready_label),
                        )
                    })
                    .child(
                        div()
                            .when(!settling, |label| label.text_color(palette::text()))
                            .child(hold_label),
                    ),
            )
    }
}

/// A circle of tongue directions as seen in a mirror: your right is on the
/// right. It shows a dot with a faint halo, rings at points still to come,
/// or a caption when there's no direction to show.
#[derive(IntoElement)]
pub struct DirectionPad {
    size: f32,
    dot: Option<(f32, f32, f32)>,
    marks: Vec<[f32; 2]>,
    halo: Option<f32>,
    caption: Option<SharedString>,
}

impl DirectionPad {
    pub fn new(size: f32) -> Self {
        Self {
            size,
            dot: None,
            marks: Vec::new(),
            halo: None,
            caption: None,
        }
    }

    /// A dot at `horizontal`, `vertical` (each -1..1) of `diameter` pixels.
    pub fn dot(mut self, horizontal: f32, vertical: f32, diameter: f32) -> Self {
        self.dot = Some((horizontal, vertical, diameter));
        self
    }

    fn marks(mut self, marks: Vec<[f32; 2]>) -> Self {
        self.marks = marks;
        self
    }

    /// A ring this many pixels outside the dot, closing in as it's about to
    /// move, in place of its faint halo.
    fn halo(mut self, halo: Option<f32>) -> Self {
        self.halo = halo;
        self
    }

    pub fn caption(mut self, caption: impl Into<SharedString>) -> Self {
        self.caption = Some(caption.into());
        self
    }
}

impl RenderOnce for DirectionPad {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let middle = self.size / 2.;
        // Where a full reach puts the dot, and the pad's edge just outside.
        let radius = self.size * 0.45;
        let edge = middle - 4.;
        let place = |horizontal: f32, vertical: f32| {
            (
                middle + horizontal.clamp(-1., 1.) * radius,
                middle - vertical.clamp(-1., 1.) * radius,
            )
        };
        let circle = |x: f32, y: f32, size: f32| {
            div()
                .absolute()
                .left(px(x - size / 2.))
                .top(px(y - size / 2.))
                .size(px(size))
                .rounded_full()
        };
        let label = |text: Cow<'static, str>, top: f32| {
            div()
                .absolute()
                .left_0()
                .top(px(top))
                .w_full()
                .flex()
                .justify_center()
                .child(
                    div()
                        .px_1()
                        .bg(palette::sunken())
                        .font_family(MONO_FONT)
                        .text_size(px(9.))
                        .line_height(px(11.))
                        .text_color(palette::text_3())
                        .child(text),
                )
        };
        let guides = self.caption.is_none();
        let marks = self.marks.iter().map(|[h, v]| {
            let (x, y) = place(*h, *v);
            circle(x, y, 18.)
                .border_2()
                .border_color(palette::line_focus())
        });
        let (dot, halo) = match self.dot {
            Some((h, v, diameter)) => {
                let (x, y) = place(h, v);
                (
                    Some(circle(x, y, diameter).bg(palette::text())),
                    Some(match self.halo {
                        Some(halo) => circle(x, y, diameter + 2. * halo)
                            .border_2()
                            .border_color(palette::text().opacity(0.7)),
                        None => circle(x, y, diameter + 20.)
                            .border_1()
                            .border_color(palette::text().opacity(0.18)),
                    }),
                )
            }
            None => (None, None),
        };
        div()
            .relative()
            .flex_none()
            .size(px(self.size))
            .child(
                circle(middle, middle, edge * 2.)
                    .border_1()
                    .border_color(palette::line_strong())
                    .bg(palette::sunken()),
            )
            .when(guides, |pad| {
                pad.child(
                    circle(middle, middle, edge)
                        .border_1()
                        .border_color(palette::line_soft()),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(middle - edge))
                        .top(px(middle))
                        .w(px(edge * 2.))
                        .h(px(1.))
                        .bg(palette::line_soft()),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(middle))
                        .top(px(middle - edge))
                        .w(px(1.))
                        .h(px(edge * 2.))
                        .bg(palette::line_soft()),
                )
                .child(label(t!("tongue.pad_up"), middle - edge + 8.))
                .child(label(t!("tongue.pad_down"), middle + edge - 19.))
            })
            .children(marks)
            .children(halo)
            .children(dot)
            .when_some(self.caption, |pad, caption| {
                pad.child(
                    div()
                        .absolute()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_lg()
                        .font_semibold()
                        .text_color(palette::text_3())
                        .child(caption),
                )
            })
    }
}

/// The phase of the running pose for its label: getting ready, recording,
/// paused or skipped.
fn phase(capture: &CaptureStatus) -> (String, Tone) {
    let remaining = capture.seconds_remaining.unwrap_or_default();
    let elapsed = (capture.step_seconds - remaining).max(0.);
    if let Some(reason) = capture.pause_reason {
        // It carries on by itself once the reason goes away.
        (
            t!(
                "tongue.phase_waiting",
                reason = crate::prompts::pause_message(reason)
            )
            .into(),
            Tone::Waiting,
        )
    } else if capture.paused {
        (t!("tongue.phase_paused").into(), Tone::Off)
    } else if capture.skipped {
        (t!("tongue.phase_skipped").into(), Tone::Off)
    } else if capture.recording {
        (
            t!("tongue.recording_for", seconds = remaining.ceil() as u32).into(),
            Tone::Good,
        )
    } else {
        (
            t!(
                "tongue.get_ready",
                seconds = ((capture.settle_seconds - elapsed).ceil() as u32).max(1)
            )
            .into(),
            Tone::Waiting,
        )
    }
}

/// Linear interpolation along the dot's `[seconds, horizontal, vertical]`
/// keyframes, held at both ends, as the daemon labels frames.
fn path_position(path: &[[f32; 3]], seconds: f32) -> [f32; 2] {
    let Some(first) = path.first() else {
        return [0., 0.];
    };
    if seconds <= first[0] {
        return [first[1], first[2]];
    }
    for pair in path.windows(2) {
        let ([t0, h0, v0], [t1, h1, v1]) = (pair[0], pair[1]);
        if seconds <= t1 {
            let blend = if t1 > t0 {
                (seconds - t0) / (t1 - t0)
            } else {
                1.
            };
            return [h0 + (h1 - h0) * blend, v0 + (v1 - v0) * blend];
        }
    }
    let last = path[path.len() - 1];
    [last[1], last[2]]
}

/// Names with repeats numbered, as follow the dot repeats its part names.
fn numbered<'a>(names: impl Iterator<Item = &'a str> + Clone) -> Vec<String> {
    let mut totals: HashMap<&str, usize> = HashMap::new();
    for name in names.clone() {
        *totals.entry(name).or_default() += 1;
    }
    let mut seen: HashMap<&str, usize> = HashMap::new();
    names
        .map(|name| {
            let count = seen.entry(name).or_default();
            *count += 1;
            if totals[name] > 1 {
                format!("{name} {count}")
            } else {
                name.to_string()
            }
        })
        .collect()
}

/// A recording by when it was made and what kind it is.
fn recording_title(recording: &Recording, now: DateTime<Local>) -> String {
    format!(
        "{} \u{b7} {}",
        created_at(&recording.id)
            .map(|ms| when(ms, now, false))
            .unwrap_or_else(|| t!("tongue.recording").into()),
        mode_name(recording.mode)
    )
}

fn model_name(model: &SavedModel) -> String {
    model
        .name
        .clone()
        .unwrap_or_else(|| t!("tongue.trained_model").into())
}

/// A trained model's line under its name: when, unless its name already
/// says, then what went into it, such as "3 recordings · 4,200 frames · GPU
/// · 9 minutes".
fn model_detail(model: &SavedModel, now: DateTime<Local>) -> String {
    let mut parts = Vec::new();
    let named_by_date = model
        .name
        .as_deref()
        .is_some_and(|name| name.starts_with("Trained "));
    if !named_by_date {
        if let Some(ms) = created_at(&model.id) {
            parts.push(t!("tongue.trained_when", when = when(ms, now, true)).into());
        }
    }
    if let Some(report) = &model.report {
        if !report.recordings.is_empty() {
            parts.push(recording_count(report.recordings.len() as u64));
        }
        if let Some(frames) = report.frames {
            parts.push(frame_count(frames));
        }
        if !report.device.is_empty() {
            parts.push(report.device.clone());
        }
        if let Some(seconds) = report.seconds {
            parts.push(duration(seconds));
        }
    }
    if parts.is_empty() {
        t!("tongue.trained_on_this_pc").into()
    } else {
        parts.join(" \u{b7} ")
    }
}

/// Tongue positions in a sentence: "tongue down", or "tongue up, tongue
/// left and tongue down".
fn positions(names: &[&str]) -> String {
    let named: Vec<Cow<'static, str>> = names.iter().map(|name| position(name)).collect();
    match named.split_last() {
        None => String::new(),
        Some((last, [])) => last.to_string(),
        Some((last, rest)) => t!(
            "tongue.list_and",
            rest = rest.join(&t!("tongue.list_separator")),
            last = last
        )
        .into(),
    }
}

/// A need as a tongue position mid-sentence: "tongue down".
fn position(need: &str) -> Cow<'static, str> {
    match need {
        "Out" => t!("tongue.position_out"),
        "In" => t!("tongue.position_in"),
        "Left" => t!("tongue.position_left"),
        "Right" => t!("tongue.position_right"),
        "Up" => t!("tongue.position_up"),
        "Down" => t!("tongue.position_down"),
        need => Cow::Owned(need.to_lowercase()),
    }
}

/// The last recording's face setup report, when the last recording was a
/// face setup.
fn last_face_setup(capture: &CaptureStatus) -> Option<&FaceSetupReport> {
    let report = capture.face_setup.as_ref()?;
    let directory = capture.directory.as_deref()?;
    std::path::Path::new(directory)
        .file_name()
        .is_some_and(|name| name.to_string_lossy() == report.recording)
        .then_some(report)
}

/// The face setup's poses that didn't pass, and why, one a line.
fn face_setup_misses(report: &FaceSetupReport) -> Option<Div> {
    let misses: Vec<_> = report.poses.iter().filter(|pose| !pose.passed).collect();
    if misses.is_empty() {
        return None;
    }
    Some(
        v_flex()
            .gap_1()
            .px_3p5()
            .child(
                div()
                    .text_xs()
                    .font_medium()
                    .text_color(palette::text_2())
                    .child(t!("tongue.face_setup_missed")),
            )
            .children(misses.into_iter().map(|pose| {
                let why = if pose.skipped {
                    t!("tongue.face_setup_skipped").into_owned()
                } else {
                    pose.reason.clone().unwrap_or_default()
                };
                div().text_xs().text_color(palette::text_3()).child(t!(
                    "tongue.face_setup_miss",
                    pose = pose.pose,
                    why = why
                ))
            })),
    )
}

/// When a recording or model was made, from the milliseconds its ID starts with.
fn created_at(id: &str) -> Option<i64> {
    id.split('-').next()?.parse().ok()
}

/// "Today 12:30", "Yesterday 12:30" or "25 Sep 12:30"; `lower` for mid-sentence.
fn when(ms: i64, now: DateTime<Local>, lower: bool) -> String {
    let Some(at) = Local.timestamp_millis_opt(ms).single() else {
        return t!("tongue.recording").into();
    };
    let time = at.format("%H:%M").to_string();
    let day = at.date_naive();
    let today = now.date_naive();
    if day == today {
        if lower {
            t!("tongue.today_lower", time = time)
        } else {
            t!("tongue.today", time = time)
        }
    } else if today.pred_opt() == Some(day) {
        if lower {
            t!("tongue.yesterday_lower", time = time)
        } else {
            t!("tongue.yesterday", time = time)
        }
    } else {
        t!("tongue.on_date", date = at.format("%-d %b"), time = time)
    }
    .into()
}

fn default_name(now: DateTime<Local>) -> String {
    format!("Trained {}", now.format("%-d %b %H:%M"))
}

/// What follows the training message: the time left, or while that isn't
/// known yet, that it's being worked out. The first batch also sets up the
/// GPU, which can take minutes while a game shares it.
fn training_time_left(progress: &TrainingProgress) -> String {
    match progress.eta_seconds {
        Some(eta) => t!("tongue.about_left", duration = duration(eta)).into(),
        None if progress.stage == TrainingStage::Training => t!("tongue.setting_up_gpu").into(),
        None => String::new(),
    }
}

/// What a download brings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Package {
    /// What training needs: the mouth-camera pair, then the training
    /// examples.
    TrainingFiles,
    /// The QFTPlus Model: the mouth-camera pair, then QFT+'s face model.
    QftPlus,
}

/// A model's download as it runs: what it's doing, a meter, how much has
/// come and how fast, and a way to stop it. Both packages fetch the
/// mouth-camera pair first, so `pair_installed` tells which part is coming.
fn download_progress(
    builtin: &BuiltinStatus,
    package: Package,
    pair_installed: bool,
    busy: bool,
    cx: &Context<TongueTraining>,
) -> AnyElement {
    let stage = builtin.stage.unwrap_or_default();
    let qftplus = pair_installed && package == Package::QftPlus;
    let examples = pair_installed && package == Package::TrainingFiles;
    let title = match stage {
        BuiltinStage::Downloading if qftplus => t!("tongue.downloading_qftplus"),
        BuiltinStage::Downloading if examples => t!("tongue.downloading_examples"),
        BuiltinStage::Downloading => t!("tongue.downloading_pair"),
        BuiltinStage::Verifying => t!("tongue.checking_download"),
        BuiltinStage::Unpacking if qftplus => t!("tongue.unpacking_qftplus"),
        BuiltinStage::Unpacking if examples => t!("tongue.unpacking_examples"),
        BuiltinStage::Unpacking => t!("tongue.unpacking_pair"),
        _ => t!("tongue.connecting_download"),
    };
    let fraction = match stage {
        BuiltinStage::Downloading => builtin.fraction.unwrap_or(0.),
        BuiltinStage::Verifying | BuiltinStage::Unpacking => 1.,
        _ => 0.,
    };
    let detail = (stage == BuiltinStage::Downloading)
        .then(|| download_detail(builtin))
        .flatten();
    v_flex()
        .w_full()
        .gap_2p5()
        .px_3p5()
        .py_3()
        .rounded(px(10.))
        .border_1()
        .border_color(palette::line_strong())
        .bg(palette::inset())
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .child(div().flex_1().min_w_0().text_sm().child(title))
                // Unpacking is over in moments and can't stop part-way.
                .when(stage != BuiltinStage::Unpacking, |row| {
                    row.child(
                        Button::new(match package {
                            Package::TrainingFiles => "cancel-builtin",
                            Package::QftPlus => "cancel-qftplus",
                        })
                        .ghost()
                        .xsmall()
                        .label(t!("tongue.cancel"))
                        .disabled(busy)
                        .on_click(cx.listener(
                            move |section, _, _, cx| match package {
                                Package::TrainingFiles => section.cancel_builtin(cx),
                                Package::QftPlus => section.cancel_qftplus(cx),
                            },
                        )),
                    )
                }),
        )
        .child(Meter::new(fraction, palette::text()))
        .children(detail.map(|detail| {
            div()
                .text_xs()
                .font_family(MONO_FONT)
                .text_color(palette::text_3())
                .child(detail)
        }))
        .into_any_element()
}

/// "45.2 of 140.1 MB · 5.3 MB/s · 18 s left", as much of it as is known.
fn download_detail(builtin: &BuiltinStatus) -> Option<String> {
    let received = builtin.received_bytes?;
    let mut parts = vec![match builtin.total_bytes {
        Some(total) => t!(
            "tongue.download_amount",
            received = megabytes(received as f64),
            total = megabytes(total as f64)
        )
        .to_string(),
        None => t!(
            "tongue.download_received",
            received = megabytes(received as f64)
        )
        .to_string(),
    }];
    if let Some(speed) = builtin.bytes_per_second.filter(|speed| *speed > 0.) {
        parts.push(t!("tongue.download_speed", speed = megabytes(speed)).to_string());
        if let Some(total) = builtin.total_bytes {
            let seconds = total.saturating_sub(received) as f64 / speed;
            parts.push(if seconds < 60. {
                t!("tongue.seconds_left", seconds = seconds.ceil() as u64).to_string()
            } else {
                t!("tongue.download_left", duration = duration(seconds)).to_string()
            });
        }
    }
    Some(parts.join(" · "))
}

/// Bytes as megabytes to one place: "45.2".
fn megabytes(bytes: f64) -> String {
    format!("{:.1}", bytes / 1_000_000.)
}

fn duration(seconds: f64) -> String {
    if !seconds.is_finite() {
        return String::new();
    }
    if seconds < 60. {
        return t!("tongue.under_a_minute").into();
    }
    let minutes = (seconds / 60.).round() as u64;
    if minutes < 60 {
        t!("tongue.minutes", minutes = minutes).into()
    } else {
        t!(
            "tongue.hours_minutes",
            hours = minutes / 60,
            minutes = minutes % 60
        )
        .into()
    }
}

/// "1 frame" or "12,345 frames".
fn frame_count(count: u64) -> String {
    if count == 1 {
        t!("tongue.frame_one", count = grouped(count))
    } else {
        t!("tongue.frame_other", count = grouped(count))
    }
    .into()
}

/// "1 recording" or "3 recordings".
fn recording_count(count: u64) -> String {
    if count == 1 {
        t!("tongue.recording_one", count = grouped(count))
    } else {
        t!("tongue.recording_other", count = grouped(count))
    }
    .into()
}

/// A count with its thousands marked: "12,345".
fn grouped(count: u64) -> String {
    let digits = count.to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// Where the recordings left out of training are kept.
fn unticked_path() -> Option<std::path::PathBuf> {
    let config = vrft_gui_core::paths::config_file()?;
    Some(config.parent()?.join(UNTICKED_FILE))
}

fn load_unticked() -> HashSet<String> {
    unticked_path()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_unticked(unticked: &HashSet<String>) {
    let Some(path) = unticked_path() else {
        return;
    };
    let mut ids: Vec<&String> = unticked.iter().collect();
    ids.sort();
    let saved = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|_| {
            std::fs::write(
                &path,
                serde_json::to_vec_pretty(&ids).unwrap_or_else(|_| b"[]".to_vec()),
            )
        });
    if let Err(error) = saved {
        log::warn!("Couldn't save which recordings are left out: {error}");
    }
}

/// What a failed training means, in plain words, from the trainer's error.
fn training_failure(error: &str) -> String {
    let lower = error.to_lowercase();
    let plain = if lower.contains("not finite") {
        t!("tongue.failure_unstable")
    } else if [
        "wgpu",
        "gpu",
        "adapter",
        "device lost",
        "out of memory",
        "vulkan",
        "dx12",
    ]
    .iter()
    .any(|word| lower.contains(word))
    {
        t!("tongue.failure_gpu")
    } else if lower.contains("at least one recording") {
        t!("tongue.failure_no_recordings")
    } else if lower.contains("base model") {
        t!("tongue.failure_training_files")
    } else {
        return t!("tongue.failure", error = error).into();
    };
    plain.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recording(coverage: Coverage) -> Recording {
        Recording {
            coverage,
            ..Recording::default()
        }
    }

    #[test]
    fn cheek_puffs_are_covered_across_the_ticked_recordings() {
        let left = recording(Coverage {
            cheek_left: 16,
            ..Coverage::default()
        });
        let right = recording(Coverage {
            cheek_right: 8,
            ..Coverage::default()
        });
        assert!(!cheeks_covered(&[&left]));
        assert!(cheeks_covered(&[&left, &right]));
        // Cheek puffs are never what stands in the way of training.
        assert!(!missing(&[&left]).iter().any(|need| need.contains("heek")));
    }

    #[test]
    fn training_needs_every_direction_across_the_ticked_recordings() {
        let basic = recording(Coverage {
            out: 40,
            inside: 24,
            left: 8,
            right: 8,
            up: 8,
            down: 4,
            ..Coverage::default()
        });
        assert_eq!(missing(&[&basic]), vec!["Down"]);
        assert_eq!(coverage(&[&basic])[5], ("Down", 4, 8));
        let extra = recording(Coverage {
            down: 4,
            ..Coverage::default()
        });
        assert!(missing(&[&basic, &extra]).is_empty());
        assert_eq!(missing(&[]).len(), 6);
        assert_eq!(positions(&["Down"]), "tongue down");
        assert_eq!(
            positions(&["Out", "Left", "Down"]),
            "tongue out, tongue left and tongue down"
        );
    }

    #[test]
    fn dates_read_as_today_yesterday_or_the_date() {
        let now = Local.with_ymd_and_hms(2026, 9, 25, 18, 0, 0).unwrap();
        let at = |d, h, m| {
            Local
                .with_ymd_and_hms(2026, 9, d, h, m, 0)
                .unwrap()
                .timestamp_millis()
        };
        assert_eq!(when(at(25, 12, 30), now, false), "Today 12:30");
        assert_eq!(when(at(25, 12, 30), now, true), "today 12:30");
        assert_eq!(when(at(24, 9, 5), now, false), "Yesterday 09:05");
        assert_eq!(when(at(3, 9, 5), now, false), "3 Sep 09:05");
        assert_eq!(created_at("1758800000000-core-42"), Some(1758800000000));
        assert_eq!(created_at("demo"), None);
    }

    #[test]
    fn counts_and_durations_read_naturally() {
        assert_eq!(frame_count(1), "1 frame");
        assert_eq!(frame_count(12345), "12,345 frames");
        assert_eq!(recording_count(3), "3 recordings");
        assert_eq!(grouped(3400), "3,400");
        assert_eq!(grouped(999), "999");
        assert_eq!(duration(30.), "under a minute");
        assert_eq!(duration(3900.), "1 h 5 min");
    }

    #[test]
    fn repeated_pose_names_are_numbered() {
        let names = ["Follow the dot", "Tongue in, relax", "Follow the dot"];
        assert_eq!(
            numbered(names.into_iter()),
            ["Follow the dot 1", "Tongue in, relax", "Follow the dot 2"]
        );
    }

    #[test]
    fn the_dot_moves_between_keyframes_and_holds_at_the_ends() {
        let path = [[0., 0., 0.], [1., 0., 0.], [2., 1., -1.]];
        assert_eq!(path_position(&path, -1.), [0., 0.]);
        assert_eq!(path_position(&path, 1.5), [0.5, -0.5]);
        assert_eq!(path_position(&path, 9.), [1., -1.]);
    }

    #[test]
    fn the_phase_counts_down_getting_ready_then_recording() {
        let mut capture = CaptureStatus {
            active: true,
            step_seconds: 8.,
            settle_seconds: 4.,
            seconds_remaining: Some(6.5),
            ..CaptureStatus::default()
        };
        assert_eq!(
            phase(&capture),
            ("Get ready \u{b7} 3 s".into(), Tone::Waiting)
        );
        capture.recording = true;
        capture.seconds_remaining = Some(2.2);
        assert_eq!(phase(&capture), ("Recording \u{b7} 3 s".into(), Tone::Good));
        capture.paused = true;
        assert_eq!(phase(&capture).0, "Paused");
        capture.pause_reason = Some(crate::daemon::PauseReason::CamerasStopped);
        assert_eq!(
            phase(&capture),
            (
                "Paused: the mouth cameras stopped, waiting".into(),
                Tone::Waiting
            )
        );
    }

    #[test]
    fn a_recording_says_roughly_how_long_it_has_left() {
        let mut capture = CaptureStatus {
            active: true,
            step: Some(7),
            total_steps: Some(14),
            step_seconds: 8.,
            seconds_remaining: Some(5.),
            ..CaptureStatus::default()
        };
        assert_eq!(time_left(&capture).as_deref(), Some("About 1 min left"));
        capture.step = Some(14);
        assert_eq!(time_left(&capture).as_deref(), Some("Under a minute left"));
        capture.total_steps = None;
        assert_eq!(time_left(&capture), None);
    }

    #[test]
    fn training_says_how_long_is_left_or_that_it_is_working_it_out() {
        let mut progress = TrainingProgress::new(TrainingStage::Training, "Learning");
        assert_eq!(
            training_time_left(&progress),
            " \u{b7} setting up the GPU and working out the time left"
        );
        progress.eta_seconds = Some(1250.);
        assert_eq!(training_time_left(&progress), " \u{b7} about 21 min left");
        let checking = TrainingProgress::new(TrainingStage::Checking, "Checking");
        assert_eq!(training_time_left(&checking), "");
    }

    #[test]
    fn training_failures_are_explained() {
        assert!(training_failure("Training loss is not finite").contains("unstable"));
        assert!(training_failure("wgpu: Device lost").contains("CPU"));
        assert_eq!(training_failure("disk full"), "Training failed: disk full");
    }
}
