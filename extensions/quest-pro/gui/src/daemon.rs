//! Client for the daemon's Quest Pro routes, served under `/ext/quest-pro`
//! on its local API. The wire types are `vrft-quest-pro-protocol`'s, which
//! the daemon half serves.
use anyhow::{bail, Context as _, Result};
use rust_i18n::t;
use std::sync::Arc;
use vrft_gui_core::client::DaemonClient;
pub use vrft_quest_pro_protocol::*;

pub struct QuestProClient {
    core: Arc<DaemonClient>,
    /// Where the daemon serves this extension's routes.
    base: String,
}

impl QuestProClient {
    pub fn new(core: Arc<DaemonClient>) -> Self {
        Self {
            core,
            base: vrft_protocol::routes::extension(ID),
        }
    }

    fn path(&self, route: &str) -> String {
        format!("{}{route}", self.base)
    }

    /// The latest frame from `camera`, or `None` before the first one arrives
    /// or when it is still `shown`, whose pixels then aren't downloaded.
    pub fn frame(&self, camera: Camera, shown: Option<u64>) -> Result<Option<Frame>> {
        let route = match camera {
            Camera::Mouth => routes::FRAME,
            Camera::Eyes => routes::EYE_FRAME,
            Camera::Brow => routes::BROW_FRAME,
        };
        let mut response = self
            .core
            .agent()
            .get(self.core.url(&self.path(route)))
            .call()
            .with_context(|| self.core.unreachable())?;
        if response.status() == 204 {
            return Ok(None);
        }
        if !response.status().is_success() {
            bail!(t!("daemon.answered", status = response.status()));
        }
        let sequence = response
            .headers()
            .get(FRAME_SEQUENCE_HEADER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok())
            .context("Camera frame has no sequence number")?;
        if shown == Some(sequence) {
            return Ok(None);
        }
        // What the pupil search found in an eye snapshot, for drawing over it.
        let pupils = response
            .headers()
            .get(PUPILS_HEADER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| serde_json::from_str(value).ok());
        let pixels = response.body_mut().read_to_vec()?;
        let mut frame = Frame::of(camera, sequence, pixels)?;
        frame.pupils = pupils;
        Ok(Some(frame))
    }

    /// Changes some Quest Pro settings and returns all of them.
    pub fn update_settings(&self, patch: &SettingsPatch) -> Result<Settings> {
        self.post(routes::SETTINGS, Some(patch))
    }

    /// Takes the last second of gaze as looking straight ahead.
    pub fn recenter_eyes(&self) -> Result<Settings> {
        self.post(routes::EYE_RECENTER, None::<&()>)
    }

    pub fn clear_eye_recenter(&self) -> Result<Settings> {
        self.post(routes::EYE_RECENTER_CLEAR, None::<&()>)
    }

    /// Tries the headset again now rather than after the daemon's backoff.
    pub fn reconnect(&self) -> Result<Status> {
        self.post(routes::RECONNECT, None::<&()>)
    }

    /// The guided tongue recording, if one is running, else how the last ended.
    pub fn capture_status(&self) -> Result<CaptureStatus> {
        self.get(routes::CAPTURE_STATUS)
    }

    /// Starts a guided tongue recording, of only `poses` when there are any.
    pub fn start_capture(&self, mode: CaptureMode, poses: Vec<String>) -> Result<CaptureStatus> {
        self.post(routes::CAPTURE_START, Some(&CaptureRequest { mode, poses }))
    }

    /// Skips, pauses (which also resumes) or stops the running recording.
    pub fn capture_command(&self, command: CaptureCommand) -> Result<CaptureStatus> {
        self.post(command.route(), None::<&()>)
    }

    /// Every tongue recording on this PC, oldest first.
    pub fn recordings(&self) -> Result<Vec<Recording>> {
        self.get(routes::TRAINING_SESSIONS)
    }

    /// Leaves the given poses of a recording out of training.
    pub fn review_recording(&self, id: &str, excluded_steps: &[u64]) -> Result<()> {
        let request = ReviewRequest {
            id: id.into(),
            excluded_steps: excluded_steps.to_vec(),
        };
        self.post::<ReviewSaved>(routes::TRAINING_REVIEW, Some(&request))
            .map(drop)
    }

    pub fn delete_recording(&self, id: &str) -> Result<()> {
        let request = RecordingId { id: id.into() };
        self.post::<RecordingDeleted>(routes::TRAINING_DELETE, Some(&request))
            .map(drop)
    }

    /// One saved camera frame of a recording.
    pub fn recorded_frame(&self, id: &str, index: u64) -> Result<Frame> {
        let mut response = self
            .core
            .agent()
            .get(self.core.url(&self.path(routes::TRAINING_FRAME)))
            .query("id", id)
            .query("index", index.to_string())
            .call()
            .with_context(|| self.core.unreachable())?;
        if !response.status().is_success() {
            let reason = response.body_mut().read_to_string().unwrap_or_default();
            if reason.is_empty() {
                bail!(t!("daemon.answered", status = response.status()));
            }
            bail!(reason);
        }
        Frame::new(index, response.body_mut().read_to_vec()?)
    }

    pub fn training_status(&self) -> Result<TrainingStatus> {
        self.get(routes::TRAINING_STATUS)
    }

    pub fn start_training(&self, request: &TrainRequest) -> Result<()> {
        self.post::<TrainingStarted>(routes::TRAINING_START, Some(request))
            .map(drop)
    }

    pub fn cancel_training(&self) -> Result<()> {
        self.post::<TrainingCancelled>(routes::TRAINING_CANCEL, None::<&()>)
            .map(drop)
    }

    /// Starts downloading the built-in tongue model pair.
    pub fn install_builtin(&self) -> Result<()> {
        self.post::<BuiltinStatus>(routes::TRAINING_BUILTIN, None::<&()>)
            .map(drop)
    }

    /// Stops downloading the built-in model.
    pub fn cancel_builtin(&self) -> Result<()> {
        self.post::<BuiltinStatus>(routes::TRAINING_BUILTIN_CANCEL, None::<&()>)
            .map(drop)
    }

    /// Starts copying a trained model and its recordings into a new folder
    /// inside `folder`; answers the export's serial.
    pub fn export_model(&self, id: &str, folder: std::path::PathBuf) -> Result<u64> {
        let request = ExportModel {
            id: id.into(),
            folder,
        };
        self.post::<TransferStatus>(routes::TRAINING_EXPORT_MODEL, Some(&request))
            .map(|status| status.serial)
    }

    /// Starts adding an exported model from its folder or a zip of it;
    /// answers the import's serial.
    pub fn import_model(&self, path: std::path::PathBuf) -> Result<u64> {
        self.post::<TransferStatus>(routes::TRAINING_IMPORT_MODEL, Some(&ImportModel { path }))
            .map(|status| status.serial)
    }

    /// The built-in tongue model and every personal one trained on this PC.
    pub fn models(&self) -> Result<Models> {
        self.get(routes::TRAINING_MODELS)
    }

    /// Deletes a trained model that isn't in use.
    pub fn delete_model(&self, id: &str) -> Result<()> {
        let request = RecordingId { id: id.into() };
        self.post::<RecordingDeleted>(routes::TRAINING_DELETE_MODEL, Some(&request))
            .map(drop)
    }

    /// Gives a trained model a new name.
    pub fn rename_model(&self, id: &str, name: &str) -> Result<SavedModel> {
        let request = RenameModel {
            id: id.into(),
            name: name.into(),
        };
        self.post(routes::TRAINING_RENAME_MODEL, Some(&request))
    }

    /// Puts a saved model into use; `demo` is the built-in one.
    pub fn activate_model(&self, id: &str) -> Result<()> {
        let request = RecordingId { id: id.into() };
        self.post::<ModelActivated>(routes::TRAINING_ACTIVATE, Some(&request))
            .map(drop)
    }

    fn get<T: serde::de::DeserializeOwned>(&self, route: &str) -> Result<T> {
        // Why a Quest Pro route isn't there.
        self.core.get(&self.path(route), &t!("daemon.missing"))
    }

    fn post<T: serde::de::DeserializeOwned>(
        &self,
        route: &str,
        body: Option<&impl serde::Serialize>,
    ) -> Result<T> {
        self.core.post(&self.path(route), body)
    }
}

/// The two stereo camera pairs the daemon serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Camera {
    /// The lower-face pair, streamed at the headset's camera frame rate.
    Mouth,
    /// The eye pair, sent as occasional snapshots.
    Eyes,
    /// The brow camera (camera 4) on its own, while the headset sends all
    /// five cameras.
    Brow,
}

impl Camera {
    /// The width and height of its image.
    pub fn size(self) -> (u32, u32) {
        match self {
            Camera::Mouth | Camera::Eyes => (FRAME_WIDTH, FRAME_HEIGHT),
            Camera::Brow => (VIEW, VIEW),
        }
    }
}

/// One 8-bit grayscale frame: a stereo pair, or the brow camera.
pub struct Frame {
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    /// For an eye snapshot, the pupils found in it, left view then right.
    pub pupils: Option<[Option<PupilMark>; 2]>,
}

impl Frame {
    /// A stereo pair's frame.
    pub fn new(sequence: u64, pixels: Vec<u8>) -> Result<Self> {
        Self::of(Camera::Mouth, sequence, pixels)
    }

    /// A frame from `camera`, which sets its size.
    pub fn of(camera: Camera, sequence: u64, pixels: Vec<u8>) -> Result<Self> {
        let (width, height) = camera.size();
        let expected = (width * height) as usize;
        if pixels.len() != expected {
            bail!(
                "Camera frame has {} bytes; expected {expected}",
                pixels.len()
            );
        }
        Ok(Self {
            sequence,
            width,
            height,
            pixels,
            pupils: None,
        })
    }

    /// The frame as BGRA, the pixel order GPUI images use.
    pub fn to_bgra(&self) -> Vec<u8> {
        let mut bgra = Vec::with_capacity(self.pixels.len() * 4);
        for &gray in &self.pixels {
            bgra.extend_from_slice(&[gray, gray, gray, u8::MAX]);
        }
        bgra
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_sit_under_the_extension() {
        let client = QuestProClient::new(Arc::new(DaemonClient::new("127.0.0.1:1")));
        assert_eq!(client.path(routes::FRAME), "/ext/quest-pro/frame");
        assert_eq!(
            client.path(CaptureCommand::Skip.route()),
            "/ext/quest-pro/capture/skip"
        );
    }

    #[test]
    fn frame_rejects_the_wrong_size_and_expands_to_bgra() {
        assert!(Frame::new(1, vec![0; 10]).is_err());
        let mut pixels = vec![0; FRAME_BYTES];
        pixels[0] = 200;
        let frame = Frame::new(7, pixels).unwrap();
        let bgra = frame.to_bgra();
        assert_eq!(bgra.len(), frame.pixels.len() * 4);
        assert_eq!(&bgra[..4], &[200, 200, 200, 255]);
        let brow = Frame::of(Camera::Brow, 8, vec![0; VIEW_BYTES]).unwrap();
        assert_eq!((brow.width, brow.height), (400, 400));
        assert!(Frame::of(Camera::Brow, 8, vec![0; FRAME_BYTES]).is_err());
    }
}
