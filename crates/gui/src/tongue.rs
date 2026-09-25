//! Tongue model: records guided tongue poses, trains a personal tongue model
//! on them here on this PC, and chooses the model in use. The daemon does the
//! work through its `/capture` and `/training` API; this drives and shows it,
//! as the preview page's Tongue tab does in a browser.
use crate::daemon::{
    CaptureStatus, Coverage, DaemonClient, Models, Recording, TrainRequest, TrainingStatus,
};
use crate::live::DaemonState;
use crate::speech::Speaker;
use crate::summary::{self, Connection, Tone};
use crate::widgets::{Meter, Notice, PageHeader, Panel, StatusPill};
use chrono::{DateTime, Local, TimeZone as _};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    h_flex, v_flex, ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _,
    StyledExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    black, div, img, px, relative, AnyElement, App, AppContext as _, Context, Entity, IntoElement,
    ObjectFit, ParentElement, Render, RenderImage, RenderOnce, SharedString, Styled,
    StyledImage as _, Subscription, Task, Window,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Below this content width the panels stack in one column.
const TWO_COLUMN_WIDTH: f32 = 760.;
/// A running recording changes pose every few seconds, so follow it closely.
const RECORDING_INTERVAL: Duration = Duration::from_millis(200);
const IDLE_INTERVAL: Duration = Duration::from_secs(1);
const DEFAULT_EPOCHS: u32 = 12;

/// The recordings the daemon can guide: its mode, name, and what it's for.
const MODES: [(&str, &str, &str); 4] = [
    (
        "core",
        "Basic poses",
        "14 poses, each with 4 s to get ready and 4 s held. About 2 min.",
    ),
    (
        "follow",
        "Follow the dot",
        "Follow a moving dot with the tip of your tongue, with short tongue-in rests between. It covers every direction, \
         including the ones in between, so it's the best way to keep adding data. About 2\u{bd} min.",
    ),
    (
        "direction",
        "Direction poses",
        "Halfway and diagonal directions. Use this if left-right or up-down jumps straight to the extremes. About 2 min.",
    ),
    (
        "negatives",
        "Expression poses",
        "Smiles, vowels and puffed cheeks, mostly with your tongue in. Use this if the tongue shows when it shouldn't. About 2 min.",
    ),
];

fn mode_name(mode: Option<&str>) -> &'static str {
    MODES
        .iter()
        .find(|(id, _, _)| Some(*id) == mode)
        .map_or("Recording", |(_, name, _)| name)
}

/// A count of usable frames, how many the trainer needs, and what it counts.
type Need = (fn(&Coverage) -> u64, u64, &'static str);

/// What the trainer needs across the ticked recordings, in usable frames.
const NEEDS: [Need; 6] = [
    (|c| c.out, 20, "tongue out"),
    (|c| c.inside, 20, "tongue in"),
    (|c| c.left, 8, "tongue left"),
    (|c| c.right, 8, "tongue right"),
    (|c| c.up, 8, "tongue up"),
    (|c| c.down, 8, "tongue down"),
];

/// What the ticked recordings don't have enough of to train.
fn missing(recordings: &[&Recording]) -> Vec<&'static str> {
    NEEDS
        .iter()
        .filter(|(count, needed, _)| {
            recordings
                .iter()
                .map(|recording| count(&recording.coverage))
                .sum::<u64>()
                < *needed
        })
        .map(|(_, _, name)| *name)
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

    fn label(self) -> &'static str {
        match self {
            Device::Automatic => "Automatic",
            Device::Cpu => "CPU",
            Device::Gpu => "GPU",
        }
    }

    fn wire(self) -> &'static str {
        match self {
            Device::Automatic => "auto",
            Device::Cpu => "cpu",
            // PyTorch reports ROCm GPUs as CUDA devices too.
            Device::Gpu => "cuda",
        }
    }
}

/// The request in flight, for its button to show it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pending {
    StartRecording(&'static str),
    Capture(&'static str),
    Activate(String),
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

fn read(client: &DaemonClient, lists: bool) -> Snapshot {
    Snapshot {
        capture: client.capture_status(),
        training: client.training_status(),
        lists: lists.then(|| (client.recordings(), client.models())),
    }
}

pub struct TongueTraining {
    daemon: Entity<DaemonState>,
    capture: Option<CaptureStatus>,
    /// When `capture` arrived, to move the follow-the-dot dot between polls.
    capture_at: Instant,
    training: Option<TrainingStatus>,
    /// Why the daemon can't record or train, such as being too old.
    unavailable: Option<SharedString>,
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
    show_extras: bool,
    device: Device,
    name: Entity<InputState>,
    epochs: Entity<InputState>,
    voice: bool,
    speaker: Speaker,
    /// The prompt last read aloud, as `directory:step`.
    spoken: Option<String>,
    /// The training job whose outcome is already shown, and whether it was
    /// seen running.
    shown_job: Option<String>,
    seen_busy: Option<String>,
    pending: Option<Pending>,
    record_error: Option<SharedString>,
    train_message: Option<(Tone, SharedString)>,
    model_message: Option<(Tone, SharedString)>,
    list_message: Option<SharedString>,
    watching: bool,
    lists_stale: bool,
    _poll: Task<()>,
    _action: Option<Task<()>>,
    _frames: Option<Task<()>>,
    _subscriptions: [Subscription; 1],
}

impl TongueTraining {
    pub fn new(daemon: Entity<DaemonState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Leave empty to name it by date and time")
        });
        let epochs =
            cx.new(|cx| InputState::new(window, cx).placeholder(DEFAULT_EPOCHS.to_string()));
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
        let subscriptions = [cx.observe(&daemon, |section, _, cx| {
            if section.watching {
                cx.notify();
            }
        })];
        Self {
            daemon,
            capture: None,
            capture_at: Instant::now(),
            training: None,
            unavailable: None,
            recordings: Vec::new(),
            models: None,
            unticked: HashSet::new(),
            reviewing: None,
            review_choice: HashMap::new(),
            frames: HashMap::new(),
            loading_frames: false,
            confirm_delete: None,
            confirm_cancel: false,
            show_extras: false,
            device: Device::Automatic,
            name,
            epochs,
            voice: true,
            speaker: Speaker::default(),
            spoken: None,
            shown_job: None,
            seen_busy: None,
            pending: None,
            record_error: None,
            train_message: None,
            model_message: None,
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

    fn online(&self, cx: &App) -> bool {
        *self.daemon.read(cx).connection() == Connection::Online
    }

    fn poll_plan(&mut self, cx: &App) -> Option<(Arc<DaemonClient>, bool)> {
        let wanted = self.watching || self.capture_active() || self.training_busy();
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
            Err(error) => self.unavailable = Some(format!("{error:#}").into()),
        }
        if let Ok(training) = snapshot.training {
            self.show_training(training);
        }
        if let Some((recordings, models)) = snapshot.lists {
            match recordings {
                Ok(recordings) => {
                    self.unticked
                        .retain(|id| recordings.iter().any(|recording| recording.id == *id));
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
                    self.list_message = Some(format!("Couldn't load recordings: {error:#}").into())
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
        let spoken = (capture.active && !capture.paused).then(|| {
            format!(
                "{}:{}",
                capture.directory.as_deref().unwrap_or_default(),
                capture.step.unwrap_or_default()
            )
        });
        if spoken.is_some() && spoken != self.spoken {
            // Follow-the-dot parts are short, so they get just their name.
            let prompt = if capture.mode.as_deref() == Some("follow") {
                capture.pose.as_deref()
            } else {
                capture.instruction.as_deref()
            };
            if let Some(prompt) = prompt {
                self.say(prompt);
            }
        } else if spoken.is_none() && self.spoken.is_some() {
            self.speaker.hush();
        }
        self.spoken = spoken;
        if finished {
            self.say(if capture.message.starts_with("Recording complete") {
                "Recording complete"
            } else {
                "Recording stopped"
            });
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
                self.train_message = match progress.stage.as_str() {
                    "complete" => {
                        if seen {
                            self.say("Training complete");
                        }
                        let switched = training.active_id == *id;
                        let mut text = if switched {
                            "Done. Your new model is switched on.".to_string()
                        } else if training.model_override {
                            "Done. VRFT_TONGUE_MODEL_DIR is set, so the new model was saved but not switched on.".to_string()
                        } else {
                            "Done. Your new model is saved.".to_string()
                        };
                        if let Some(report) = &progress.report {
                            text.push_str(&format!(
                                " Trained on {} in {}.",
                                plural(report.recordings.len() as u64, "recording"),
                                report.seconds.map(duration).unwrap_or_default()
                            ));
                        }
                        text.push_str(" Stick your tongue out and move it around: the dot above should follow. Not tracking well? Refit the headset, record another set of basic poses and train again.");
                        Some((Tone::Good, text.into()))
                    }
                    "failed" => Some((
                        Tone::Problem,
                        format!("Training failed: {}", progress.message).into(),
                    )),
                    "cancelled" => Some((
                        Tone::Off,
                        "Training cancelled. The model in use hasn't changed.".into(),
                    )),
                    _ => None,
                };
                self.lists_stale = true;
            }
        }
        if let Some(models) = &mut self.models {
            models.active_id = training.active_id.clone();
        }
        self.training = Some(training);
    }

    fn say(&mut self, text: &str) {
        if self.voice {
            self.speaker.say(text);
        }
    }

    /// Runs one request off the UI thread, then `done` with its result.
    fn request<T: Send + 'static>(
        &mut self,
        pending: Pending,
        work: impl FnOnce(&DaemonClient) -> anyhow::Result<T> + Send + 'static,
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

    fn start_recording(&mut self, mode: &'static str, cx: &mut Context<Self>) {
        self.record_error = None;
        self.show_extras = false;
        self.request(
            Pending::StartRecording(mode),
            move |client| client.start_capture(mode),
            Self::capture_done,
            cx,
        );
    }

    fn capture_command(&mut self, command: &'static str, cx: &mut Context<Self>) {
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
            Err(error) => self.record_error = Some(format!("{error:#}").into()),
        }
    }

    fn set_voice(&mut self, on: bool, cx: &mut Context<Self>) {
        self.voice = on;
        if !on {
            self.speaker.hush();
        }
        // Read the current prompt again once it's switched back on.
        self.spoken = None;
        cx.notify();
    }

    fn activate(&mut self, id: String, cx: &mut Context<Self>) {
        self.model_message = None;
        let request = id.clone();
        self.request(
            Pending::Activate(id.clone()),
            move |client| client.activate_model(&request),
            move |section, result, _| match result {
                Ok(()) => {
                    if let Some(models) = &mut section.models {
                        models.active_id = id;
                    }
                    section.model_message = Some((
                        Tone::Good,
                        "Switched. Tracking uses it within a second.".into(),
                    ));
                }
                Err(error) => {
                    section.model_message = Some((Tone::Problem, format!("{error:#}").into()))
                }
            },
            cx,
        );
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
        cx.notify();
    }

    fn start_training(&mut self, cx: &mut Context<Self>) {
        let typed_name = self.name.read(cx).value().trim().to_string();
        let name = if typed_name.is_empty() {
            default_name(Local::now())
        } else {
            typed_name.chars().take(100).collect()
        };
        let epochs = self
            .epochs
            .read(cx)
            .value()
            .trim()
            .parse::<u32>()
            .unwrap_or(DEFAULT_EPOCHS)
            .clamp(1, 60);
        let request = TrainRequest {
            name,
            recordings: self.ticked().iter().map(|r| r.id.clone()).collect(),
            device: self.device.wire(),
            epochs,
        };
        self.train_message = None;
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
                    section.train_message = Some((
                        Tone::Problem,
                        format!("Couldn't start training: {error:#}").into(),
                    ))
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
                    section.train_message = Some((Tone::Problem, format!("{error:#}").into()));
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
                        section.unticked.remove(&id);
                        section.recordings.retain(|recording| recording.id != id);
                        if section.reviewing.as_ref() == Some(&id) {
                            section.close_review(cx);
                        }
                    }
                    Err(error) => {
                        section.list_message = Some(format!("Couldn't delete: {error:#}").into())
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
            for index in wanted {
                let (client, id_for_request) = (client.clone(), id.clone());
                let image = cx
                    .background_executor()
                    .spawn(async move {
                        client
                            .recorded_frame(&id_for_request, index)
                            .map(|frame| crate::live::decode(frame).1)
                    })
                    .await;
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
                                Some(format!("Couldn't show a recorded frame: {error:#}").into())
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
                    section.list_message = Some(format!("Couldn't save: {error:#}").into());
                }
                // Coverage changes with the poses left out.
                section.lists_stale = true;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn model_panel(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let status = self.daemon.read(cx).status();
        let reading = summary::tongue_model(status);
        let training = self.training.as_ref();
        let locked = self.training_busy() || self.capture_active();
        let override_set = training.is_some_and(|training| training.model_override);
        let active = self
            .models
            .as_ref()
            .map(|models| models.active_id.as_str())
            .unwrap_or("demo");
        let mut models: Vec<_> = self
            .models
            .as_ref()
            .map(|models| models.models.iter().filter(|m| m.id != "demo").collect())
            .unwrap_or_default();
        models.sort_by_key(|model| std::cmp::Reverse(created_at(&model.id)));
        let now = Local::now();
        let rows = std::iter::once((
            "demo".to_string(),
            "Built-in model".to_string(),
            "Not trained on you".to_string(),
        ))
        .chain(models.into_iter().map(|model| {
            let trained = created_at(&model.id)
                .map(|ms| format!("Trained {}", when(ms, now, true)))
                .unwrap_or_else(|| "Trained on this PC".into());
            (
                model.id.clone(),
                model.name.clone().unwrap_or_else(|| "Trained model".into()),
                trained,
            )
        }))
        .enumerate()
        .map(|(index, (id, name, detail))| {
            let in_use = id == active;
            let activating = self.pending == Some(Pending::Activate(id.clone()));
            h_flex()
                .gap_3()
                .justify_between()
                .py_1p5()
                .when(index > 0, |row| row.border_t_1().border_color(theme.border))
                .child(
                    v_flex()
                        .min_w_0()
                        .child(div().text_sm().font_medium().truncate().child(name))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(detail),
                        ),
                )
                .child(if in_use {
                    StatusPill::new(Tone::Good, "In use").into_any_element()
                } else {
                    Button::new(("use-model", index))
                        .outline()
                        .small()
                        .label("Use")
                        .loading(activating)
                        .disabled(locked || override_set || self.pending.is_some())
                        .on_click(
                            cx.listener(move |section, _, _, cx| section.activate(id.clone(), cx)),
                        )
                        .into_any_element()
                })
                .into_any_element()
        });
        Panel::new("Model in use")
            .child(Notice::new(
                reading.tone,
                if reading.detail.is_empty() {
                    format!("Tongue model: {}", reading.value.to_lowercase())
                } else {
                    format!(
                        "Tongue model: {}. {}",
                        reading.value.to_lowercase(),
                        reading.detail
                    )
                },
            ))
            .child(v_flex().children(rows))
            .when(override_set, |panel| {
                panel.child(Notice::new(
                    Tone::Waiting,
                    "VRFT_TONGUE_MODEL_DIR sets the model, so it can't be changed here.",
                ))
            })
            .when(locked && !override_set, |panel| {
                panel.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("You can switch models once the recording or training finishes."),
                )
            })
            .children(
                self.model_message
                    .clone()
                    .map(|(tone, text)| Notice::new(tone, text)),
            )
            .into_any_element()
    }

    /// Why a recording can't start now.
    fn blockers(&self, cx: &App) -> Vec<(&'static str, &'static str)> {
        let status = self.daemon.read(cx).status();
        let mut blockers = Vec::new();
        if !status.is_some_and(summary::camera_live) {
            blockers.push((
                "The mouth cameras aren't sending anything.",
                "Start the headset app's stream on the Headset page, then put the headset on.",
            ));
        }
        if !self
            .capture
            .as_ref()
            .is_some_and(|capture| capture.native_recent)
        {
            blockers.push((
                "VRFT isn't receiving the headset's face tracking.",
                "Run VRFT normally (not with --camera-preview-only), with Virtual Desktop streaming face tracking.",
            ));
        }
        if self.training_busy() {
            blockers.push((
                "Training is running.",
                "You can record again once it finishes.",
            ));
        }
        blockers
    }

    fn record_panel(&self, cx: &Context<Self>) -> AnyElement {
        if let Some(capture) = self.capture.as_ref().filter(|capture| capture.active) {
            return self.prompt_panel(capture, cx);
        }
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let blockers = self.blockers(cx);
        let ready = blockers.is_empty() && self.pending.is_none();
        let outcome = self
            .capture
            .as_ref()
            .filter(|capture| !capture.message.is_empty())
            .map(|capture| {
                let complete = capture.message.starts_with("Recording complete");
                let text = if complete && !self.training_busy() {
                    format!("{} Now train your model.", capture.message)
                } else {
                    capture.message.clone()
                };
                Notice::new(if complete { Tone::Good } else { Tone::Waiting }, text)
            });
        let extras = MODES[1..].iter().map(|(mode, name, about)| {
            h_flex()
                .gap_3()
                .justify_between()
                .items_start()
                .child(
                    v_flex()
                        .min_w_0()
                        .gap_0p5()
                        .child(div().text_sm().font_medium().child(*name))
                        .child(div().text_xs().text_color(muted).child(*about)),
                )
                .child(
                    Button::new(SharedString::from(format!("record-{mode}")))
                        .outline()
                        .small()
                        .label("Record")
                        .loading(self.pending == Some(Pending::StartRecording(mode)))
                        .disabled(!ready)
                        .on_click(
                            cx.listener(|section, _, _, cx| section.start_recording(mode, cx)),
                        ),
                )
        });
        Panel::new("1. Record")
            .child(div().text_sm().child(
                "Put the headset on and keep this page in view. 14 poses, each with 4 s to get ready and 4 s held, about 2 minutes in all.",
            ))
            .child(div().text_xs().text_color(muted).child(
                "Do what each prompt says, such as \"half out\", rather than going to the extreme. Keep your tongue where both cameras can see it.",
            ))
            .children(blockers.into_iter().map(|(problem, fix)| {
                v_flex()
                    .gap_0p5()
                    .child(Notice::new(Tone::Problem, problem))
                    .child(div().pl_4().text_xs().text_color(muted).child(fix))
            }))
            .child(
                h_flex()
                    .gap_4()
                    .flex_wrap()
                    .child(
                        Button::new("record-core")
                            .primary()
                            .icon(IconName::Play)
                            .label("Start recording")
                            .loading(self.pending == Some(Pending::StartRecording("core")))
                            .disabled(!ready)
                            .on_click(cx.listener(|section, _, _, cx| section.start_recording("core", cx))),
                    )
                    .child(
                        Switch::new("read-aloud")
                            .label("Read prompts aloud")
                            .checked(self.voice)
                            .on_change(cx.listener(|section, on: &bool, _, cx| section.set_voice(*on, cx))),
                    ),
            )
            .children(outcome)
            .children(self.record_error.clone().map(|error| Notice::new(Tone::Problem, error)))
            .child(div().h(px(1.)).bg(theme.border))
            .child(
                Button::new("show-extras")
                    .ghost()
                    .small()
                    .icon(if self.show_extras {
                        IconName::ChevronUp
                    } else {
                        IconName::ChevronDown
                    })
                    .label("Extra recordings (optional)")
                    .on_click(cx.listener(|section, _, _, cx| {
                        section.show_extras = !section.show_extras;
                        cx.notify();
                    })),
            )
            .when(self.show_extras, |panel| {
                panel
                    .child(div().text_xs().text_color(muted).child(
                        "Use these if something still tracks badly after training. They add to your basic recording rather than replace it.",
                    ))
                    .child(v_flex().gap_3().children(extras))
            })
            .into_any_element()
    }

    /// The running recording: the pose to do, its countdown, and controls.
    fn prompt_panel(&self, capture: &CaptureStatus, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let follow = capture.mode.as_deref() == Some("follow");
        let step_seconds = capture.step_seconds.max(0.1);
        let elapsed = (step_seconds - capture.seconds_remaining.unwrap_or(step_seconds)).max(0.);
        let (phase, tone) = phase(capture);
        let step = capture.step.unwrap_or(1);
        let total = capture.total_steps.unwrap_or(1).max(1);
        let progress = ((step - 1) as f32 + elapsed / step_seconds) / total as f32;
        let settle = (capture.settle_seconds / step_seconds).clamp(0., 1.);
        let part = if follow { "part" } else { "pose" };
        let next = match &capture.next_pose {
            Some(next) => format!("Up next: {next}"),
            None => format!("Last {part}"),
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
                None => DirectionPad::new(280.).caption("Tongue in"),
            };
            div().flex().justify_center().child(pad)
        });
        let busy = self.pending.is_some();
        Panel::new("1. Record")
            .child(
                h_flex()
                    .justify_between()
                    .gap_2()
                    .child(div().text_sm().text_color(theme.muted_foreground).child(format!(
                        "{} \u{b7} {part} {step} of {total}",
                        mode_name(capture.mode.as_deref())
                    )))
                    .child(StatusPill::new(tone, phase)),
            )
            .child(Meter::new(progress, theme.primary))
            .child(
                div()
                    .text_2xl()
                    .font_semibold()
                    .child(capture.pose.clone().unwrap_or_default()),
            )
            .child(div().text_base().child(capture.instruction.clone().unwrap_or_default()))
            .children(pad)
            .child(PoseTimer {
                settle,
                elapsed: (elapsed / step_seconds).clamp(0., 1.),
            })
            .child(div().text_sm().text_color(theme.muted_foreground).child(next))
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("pause-recording")
                            .outline()
                            .icon(if capture.paused {
                                IconName::Play
                            } else {
                                IconName::Pause
                            })
                            .label(if capture.paused { "Resume" } else { "Pause" })
                            .loading(self.pending == Some(Pending::Capture("pause")))
                            .disabled(busy)
                            .on_click(cx.listener(|section, _, _, cx| section.capture_command("pause", cx))),
                    )
                    .child(
                        Button::new("skip-pose")
                            .outline()
                            .icon(IconName::SkipForward)
                            .label(if follow { "Skip this part" } else { "Skip this pose" })
                            .loading(self.pending == Some(Pending::Capture("skip")))
                            .disabled(busy || capture.skipped)
                            .on_click(cx.listener(|section, _, _, cx| section.capture_command("skip", cx))),
                    )
                    .child(
                        Button::new("stop-recording")
                            .danger()
                            .icon(IconName::CircleStop)
                            .label("Stop")
                            .loading(self.pending == Some(Pending::Capture("stop")))
                            .disabled(busy)
                            .on_click(cx.listener(|section, _, _, cx| section.capture_command("stop", cx))),
                    )
                    .child(
                        Switch::new("read-aloud-recording")
                            .label("Read prompts aloud")
                            .checked(self.voice)
                            .on_change(cx.listener(|section, on: &bool, _, cx| section.set_voice(*on, cx))),
                    ),
            )
            .child(div().text_xs().text_color(theme.muted_foreground).child(if follow {
                "Lost the dot, or your tongue is out of view? Skip this part. If you stop, everything recorded so far is kept."
            } else {
                "Can't do a pose, or your tongue is out of view? Skip it. If you stop, everything recorded so far is kept."
            }))
            .children(self.record_error.clone().map(|error| Notice::new(Tone::Problem, error)))
            .into_any_element()
    }

    fn train_panel(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let busy = self.training_busy();
        let recording = self.capture_active();
        let ticked = self.ticked();
        let lacking = missing(&ticked);
        let summary: Option<(Tone, String)> = if busy {
            None
        } else if recording {
            Some((
                Tone::Off,
                "Finish recording first. The new recording will appear here.".into(),
            ))
        } else if !self.recordings.iter().any(|r| r.error.is_none()) {
            Some((Tone::Off, "Make a recording first.".into()))
        } else if ticked.is_empty() {
            Some((
                Tone::Waiting,
                "Tick at least one recording to train on.".into(),
            ))
        } else if !lacking.is_empty() {
            Some((
                Tone::Waiting,
                format!(
                    "The ticked recordings don't have enough {}. Record a full set of basic poses, or tick a recording that has them.",
                    listed(&lacking)
                ),
            ))
        } else {
            let frames = ticked.iter().map(|r| r.frames).sum();
            Some((
                Tone::Good,
                format!(
                    "Ready to train on {} ({}).",
                    plural(ticked.len() as u64, "recording"),
                    plural(frames, "frame")
                ),
            ))
        };
        let can_train = !busy
            && !recording
            && self.pending.is_none()
            && !ticked.is_empty()
            && lacking.is_empty();
        let progress = self
            .training
            .as_ref()
            .filter(|training| training.busy)
            .map(|training| {
                let progress = training.progress.clone().unwrap_or_default();
                let fraction = progress.fraction.unwrap_or(0.).clamp(0., 1.);
                let message = if progress.message.is_empty() {
                    "Starting\u{2026}".to_string()
                } else {
                    progress.message
                };
                let left = progress
                    .eta_seconds
                    .map(|eta| format!(" \u{b7} about {} left", duration(eta)))
                    .unwrap_or_default();
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .justify_between()
                            .gap_2()
                            .text_sm()
                            .child(format!("{message}{left}"))
                            .child(div().font_medium().child(format!("{:.0}%", fraction * 100.))),
                    )
                    .child(Meter::new(fraction, theme.primary))
                    .child(div().text_xs().text_color(muted).child(
                        "You can keep using VRFT meanwhile. Your current model stays on until the new one is ready.",
                    ))
            });
        let now = Local::now();
        let rows: Vec<AnyElement> = self
            .recordings
            .iter()
            .rev()
            .enumerate()
            .map(|(index, recording)| self.recording_row(index, recording, now, cx))
            .collect();
        let actions = if busy {
            if self.confirm_cancel {
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        div()
                            .text_sm()
                            .child("Cancel training? The model in use stays the same."),
                    )
                    .child(
                        Button::new("confirm-cancel")
                            .danger()
                            .small()
                            .label("Cancel training")
                            .loading(self.pending == Some(Pending::Cancel))
                            .on_click(cx.listener(|section, _, _, cx| section.cancel_training(cx))),
                    )
                    .child(
                        Button::new("keep-training")
                            .ghost()
                            .small()
                            .label("Keep training")
                            .on_click(cx.listener(|section, _, _, cx| {
                                section.confirm_cancel = false;
                                cx.notify();
                            })),
                    )
            } else {
                h_flex().child(
                    Button::new("cancel-training")
                        .outline()
                        .label("Cancel training")
                        .on_click(cx.listener(|section, _, _, cx| {
                            section.confirm_cancel = true;
                            cx.notify();
                        })),
                )
            }
        } else {
            h_flex().child(
                Button::new("train")
                    .primary()
                    .label("Train my model")
                    .loading(self.pending == Some(Pending::Train))
                    .disabled(!can_train)
                    .on_click(cx.listener(|section, _, _, cx| section.start_training(cx))),
            )
        };
        Panel::new("2. Train")
            .child(div().text_sm().child(
                "Trains a tongue model on your recordings, here on this PC. It switches on as soon as it's ready.",
            ))
            .child(div().text_xs().font_medium().text_color(muted).child("Recordings to train on"))
            .when(rows.is_empty(), |panel| {
                panel.child(div().text_sm().text_color(muted).child("No recordings yet."))
            })
            .child(v_flex().gap_2().children(rows))
            .children(self.list_message.clone().map(|text| Notice::new(Tone::Problem, text)))
            .children(summary.map(|(tone, text)| Notice::new(tone, text)))
            .children(progress)
            .child(actions)
            .children(
                self.train_message
                    .clone()
                    .map(|(tone, text)| Notice::new(tone, text)),
            )
            .child(div().h(px(1.)).bg(theme.border))
            .child(div().text_xs().font_medium().text_color(muted).child("Training options"))
            .child(
                v_flex()
                    .gap_2()
                    .child(option_row("Model name", Input::new(&self.name).small(), cx))
                    .child(option_row(
                        "Device",
                        h_flex().gap_1().children(Device::ALL.into_iter().map(|device| {
                            Button::new(("device", device as usize))
                                .small()
                                .outline()
                                .selected(self.device == device)
                                .label(device.label())
                                .on_click(cx.listener(move |section, _, _, cx| {
                                    section.device = device;
                                    cx.notify();
                                }))
                        })),
                        cx,
                    ))
                    .child(option_row(
                        "Training passes",
                        div().w(px(80.)).child(Input::new(&self.epochs).small()),
                        cx,
                    )),
            )
            .child(div().text_xs().text_color(muted).child(
                "Automatic uses a GPU if the installed PyTorch supports one, otherwise the CPU. \
                 More passes take longer; 12 is usually enough.",
            ))
            .into_any_element()
    }

    fn recording_row(
        &self,
        index: usize,
        recording: &Recording,
        now: DateTime<Local>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let id = recording.id.clone();
        let readable = recording.error.is_none();
        let (tag, tone) = if !readable {
            ("Unreadable", Tone::Problem)
        } else if recording.basic_ready {
            ("Complete", Tone::Good)
        } else if recording.mode.as_deref() == Some("core") {
            ("Incomplete", Tone::Waiting)
        } else {
            ("Extra", Tone::Off)
        };
        let title = format!(
            "{} \u{b7} {}",
            created_at(&recording.id)
                .map(|ms| when(ms, now, false))
                .unwrap_or_else(|| "Recording".into()),
            mode_name(recording.mode.as_deref())
        );
        let detail = recording
            .error
            .clone()
            .unwrap_or_else(|| plural(recording.frames, "frame"));
        let reviewing = self.reviewing.as_ref() == Some(&recording.id);
        let locked = self.training_busy() || self.capture_active() || self.pending.is_some();
        let actions = if self.confirm_delete.as_ref() == Some(&recording.id) {
            h_flex()
                .gap_1()
                .child(
                    Button::new(("confirm-delete", index))
                        .danger()
                        .xsmall()
                        .label("Delete")
                        .disabled(locked)
                        .on_click(cx.listener({
                            let id = id.clone();
                            move |section, _, _, cx| section.delete(id.clone(), cx)
                        })),
                )
                .child(
                    Button::new(("keep", index))
                        .ghost()
                        .xsmall()
                        .label("Keep")
                        .on_click(cx.listener(|section, _, _, cx| {
                            section.confirm_delete = None;
                            cx.notify();
                        })),
                )
        } else {
            h_flex()
                .gap_1()
                .when(readable, |row| {
                    row.child(
                        Button::new(("review", index))
                            .ghost()
                            .xsmall()
                            .label(if reviewing { "Hide" } else { "Review" })
                            .tooltip("Look through each pose and leave out any that went wrong")
                            .on_click(cx.listener({
                                let id = id.clone();
                                move |section, _, _, cx| section.toggle_review(id.clone(), cx)
                            })),
                    )
                })
                .child(
                    Button::new(("delete", index))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Trash)
                        .tooltip("Delete recording")
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
        let ticked = readable && !self.unticked.contains(&recording.id);
        v_flex()
            .gap_2()
            .p_2()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(
                h_flex()
                    .gap_2()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap_2()
                            .min_w_0()
                            .child(
                                Checkbox::new(("tick", index))
                                    .checked(ticked)
                                    .disabled(!readable || self.training_busy())
                                    .tooltip("Train on this recording")
                                    .on_click(cx.listener({
                                        let id = id.clone();
                                        move |section, checked: &bool, _, cx| {
                                            section.tick(id.clone(), *checked, cx)
                                        }
                                    })),
                            )
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .child(div().text_sm().font_medium().truncate().child(title))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .truncate()
                                            .child(detail),
                                    ),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_none()
                            .child(StatusPill::new(tone, tag))
                            .child(actions),
                    ),
            )
            .when(self.confirm_delete.as_ref() == Some(&recording.id), |row| {
                row.child(
                    div().text_xs().text_color(theme.muted_foreground).child(
                        "Delete this recording? Its camera frames are removed from this PC.",
                    ),
                )
            })
            .when(reviewing, |row| row.child(self.review(recording, cx)))
            .into_any_element()
    }

    /// Each pose of a recording, to leave out any that went wrong.
    fn review(&self, recording: &Recording, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let names = numbered(recording.poses.iter().map(|pose| pose.name.as_str()));
        let locked = self.training_busy() || self.capture_active();
        let tiles = recording.poses.iter().zip(names).map(|(pose, name)| {
            let choice = self.review_choice.get(&pose.step).copied().unwrap_or(1);
            let image = self
                .frames
                .get(&(recording.id.clone(), pose.indices[choice]))
                .cloned();
            let (id, step) = (recording.id.clone(), pose.step);
            let label = if pose.skipped {
                format!("{name} (skipped)")
            } else {
                name
            };
            v_flex()
                .gap_1()
                .min_w_0()
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
                    div()
                        .w_full()
                        .aspect_ratio(2.)
                        .rounded(theme.radius)
                        .overflow_hidden()
                        .bg(black())
                        .when(pose.excluded, |frame| frame.opacity(0.4))
                        .when_some(image, |frame, image| {
                            frame.child(img(image).size_full().object_fit(ObjectFit::Contain))
                        }),
                )
                .child(
                    h_flex().gap_1().children(
                        ["Start", "Middle", "End"]
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
                )
        });
        v_flex()
            .gap_2()
            .child(div().text_xs().text_color(theme.muted_foreground).child(
                "Untick any pose that went wrong, such as your tongue being out of view. This is optional.",
            ))
            .child(div().grid().grid_cols(3).gap_3().children(tiles))
            .into_any_element()
    }
}

impl Render for TongueTraining {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = PageHeader::new(
            "Tongue model",
            "Record your tongue, train a model on it here on this PC, and choose which model tracks you.",
        );
        if !self.online(cx) {
            return v_flex().gap_4().child(header).child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("VRFT needs to be running to record, train or switch tongue models."),
            );
        }
        if let Some(reason) = self.unavailable.clone() {
            return v_flex()
                .gap_4()
                .child(header)
                .child(Notice::new(Tone::Problem, reason));
        }
        let recording = self.capture_active();
        // Move the follow-the-dot dot smoothly between polls.
        if self
            .capture
            .as_ref()
            .is_some_and(|capture| capture.active && !capture.paused && capture.path.is_some())
        {
            window.request_animation_frame();
        }
        let two_columns = !recording && crate::shell::content_width(window) >= TWO_COLUMN_WIDTH;
        let active_name = self.models.as_ref().and_then(|models| {
            models
                .models
                .iter()
                .find(|model| model.id == models.active_id)
                .map(|model| {
                    if model.id == "demo" {
                        "Built-in model".to_string()
                    } else {
                        model.name.clone().unwrap_or_else(|| "Trained model".into())
                    }
                })
        });
        v_flex()
            .gap_4()
            .child(header.trailing(StatusPill::new(
                if self.training_busy() {
                    Tone::Waiting
                } else {
                    Tone::Good
                },
                if self.training_busy() {
                    "Training".to_string()
                } else {
                    active_name.unwrap_or_else(|| "Built-in model".into())
                },
            )))
            .child(self.model_panel(cx))
            .child(
                div()
                    .grid()
                    .grid_cols(if two_columns { 2 } else { 1 })
                    .gap_4()
                    .child(self.record_panel(cx))
                    .child(self.train_panel(cx)),
            )
    }
}

fn option_row(label: &'static str, control: impl IntoElement, cx: &App) -> impl IntoElement {
    h_flex()
        .gap_3()
        .child(
            div()
                .w(px(120.))
                .flex_none()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(div().flex_1().min_w_0().child(control))
}

/// The two parts of a pose, getting ready then recording, and how far along
/// it is.
#[derive(IntoElement)]
struct PoseTimer {
    /// Fraction of the pose spent getting ready.
    settle: f32,
    elapsed: f32,
}

impl RenderOnce for PoseTimer {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        v_flex()
            .gap_1()
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(8.))
                    .rounded_full()
                    .overflow_hidden()
                    .flex()
                    .child(
                        div()
                            .h_full()
                            .w(relative(self.settle))
                            .bg(theme.warning.opacity(0.3)),
                    )
                    .child(div().h_full().flex_1().bg(theme.success.opacity(0.3)))
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .h_full()
                            .w(relative(self.elapsed))
                            .bg(theme.foreground.opacity(0.45)),
                    ),
            )
            .child(
                div()
                    .relative()
                    .h(px(16.))
                    .text_xs()
                    .text_color(muted)
                    .child(div().absolute().left_0().child("Get ready"))
                    .child(
                        div()
                            .absolute()
                            .left(relative(self.settle))
                            .pl_1()
                            .child("Recording"),
                    ),
            )
    }
}

/// A square of tongue directions as seen in a mirror: your right is on the
/// right. It shows a dot, rings at points still to come, or a caption.
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

    /// A ring this many pixels outside the dot, closing in as it's about to move.
    fn halo(mut self, halo: Option<f32>) -> Self {
        self.halo = halo;
        self
    }

    fn caption(mut self, caption: impl Into<SharedString>) -> Self {
        self.caption = Some(caption.into());
        self
    }
}

impl RenderOnce for DirectionPad {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let middle = self.size / 2.;
        let radius = self.size * 0.45;
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
        let label = |text: &'static str| {
            div()
                .absolute()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(text)
        };
        let guides = self.caption.is_none();
        let marks = self.marks.iter().map(|[h, v]| {
            let (x, y) = place(*h, *v);
            circle(x, y, 18.)
                .border_2()
                .border_color(theme.chart_2.opacity(0.5))
        });
        let (dot, halo) = match self.dot {
            Some((h, v, diameter)) => {
                let (x, y) = place(h, v);
                (
                    Some(circle(x, y, diameter).bg(theme.chart_2)),
                    self.halo.map(|halo| {
                        circle(x, y, diameter + 2. * halo)
                            .border_2()
                            .border_color(theme.chart_2)
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
                circle(middle, middle, radius * 2.)
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background),
            )
            .when(guides, |pad| {
                pad.child(
                    circle(middle, middle, radius)
                        .border_1()
                        .border_dashed()
                        .border_color(theme.border),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(middle - radius))
                        .top(px(middle))
                        .w(px(radius * 2.))
                        .h(px(1.))
                        .bg(theme.border),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(middle))
                        .top(px(middle - radius))
                        .w(px(1.))
                        .h(px(radius * 2.))
                        .bg(theme.border),
                )
                .child(
                    label("up")
                        .top(px(middle - radius + 6.))
                        .left(px(middle + 6.)),
                )
                .child(
                    label("down")
                        .bottom(px(middle - radius + 6.))
                        .left(px(middle + 6.)),
                )
                .child(
                    label("left")
                        .left(px(middle - radius + 8.))
                        .top(px(middle - 20.)),
                )
                .child(
                    label("right")
                        .right(px(middle - radius + 8.))
                        .top(px(middle - 20.)),
                )
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
                        .text_color(theme.muted_foreground)
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
    if capture.paused {
        ("Paused".into(), Tone::Off)
    } else if capture.skipped {
        ("Skipped".into(), Tone::Off)
    } else if capture.recording {
        (
            format!("Recording \u{b7} {} s", remaining.ceil() as u32),
            Tone::Good,
        )
    } else {
        (
            format!(
                "Get ready \u{b7} {} s",
                ((capture.settle_seconds - elapsed).ceil() as u32).max(1)
            ),
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

/// When a recording or model was made, from the milliseconds its ID starts with.
fn created_at(id: &str) -> Option<i64> {
    id.split('-').next()?.parse().ok()
}

/// "Today 12:30", "Yesterday 12:30" or "25 Sep 12:30"; `lower` for mid-sentence.
fn when(ms: i64, now: DateTime<Local>, lower: bool) -> String {
    let Some(at) = Local.timestamp_millis_opt(ms).single() else {
        return "Recording".into();
    };
    let time = at.format("%H:%M");
    let day = at.date_naive();
    let today = now.date_naive();
    let named = if day == today {
        Some("Today")
    } else if today.pred_opt() == Some(day) {
        Some("Yesterday")
    } else {
        None
    };
    match named {
        Some(name) if lower => format!("{} {time}", name.to_lowercase()),
        Some(name) => format!("{name} {time}"),
        None => format!("{} {time}", at.format("%-d %b")),
    }
}

fn default_name(now: DateTime<Local>) -> String {
    format!("Trained {}", now.format("%-d %b %H:%M"))
}

fn duration(seconds: f64) -> String {
    if !seconds.is_finite() {
        return String::new();
    }
    if seconds < 60. {
        return "under a minute".into();
    }
    let minutes = (seconds / 60.).round() as u64;
    if minutes < 60 {
        format!("{minutes} min")
    } else {
        format!("{} h {} min", minutes / 60, minutes % 60)
    }
}

fn plural(count: u64, word: &str) -> String {
    let digits = count.to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    format!("{grouped} {word}{}", if count == 1 { "" } else { "s" })
}

fn listed(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
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
    fn training_needs_every_direction_across_the_ticked_recordings() {
        let basic = recording(Coverage {
            out: 40,
            inside: 24,
            left: 8,
            right: 8,
            up: 8,
            down: 4,
        });
        assert_eq!(missing(&[&basic]), vec!["tongue down"]);
        let extra = recording(Coverage {
            down: 4,
            ..Coverage::default()
        });
        assert!(missing(&[&basic, &extra]).is_empty());
        assert_eq!(missing(&[]).len(), 6);
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
    fn counts_and_lists_read_naturally() {
        assert_eq!(plural(1, "frame"), "1 frame");
        assert_eq!(plural(12345, "frame"), "12,345 frames");
        assert_eq!(listed(&["a", "b", "c"]), "a, b and c");
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
    }
}
