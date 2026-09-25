//! User-adjustable Quest Pro tongue and eye settings, saved in `.local/`.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

const FILE: &str = ".local/quest-pro-settings.json";

/// How the camera model's visibility confidence combines with the tracking
/// module's native TongueOut before the show/hide threshold is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisibilityMode {
    /// `w * camera + (1 - w) * native`, with `w` from the gate checkpoint.
    #[default]
    Weighted,
    /// Camera confidence only; ignores native TongueOut.
    Camera,
    /// Native TongueOut only. Direction still comes from the cameras.
    Native,
    /// `min(camera, native)`: fewer false positives, more misses.
    Agreement,
}

/// Per-eye yaw/pitch offsets in degrees, captured by "recenter" while the
/// wearer looks at a distant point straight ahead.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EyeOffsets {
    pub left_deg: [f64; 2],
    pub right_deg: [f64; 2],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuestProSettings {
    /// 0 is most responsive, 100 smoothest. Matches the reference hub slider.
    pub tongue_smoothing: f32,
    pub tongue_visibility: VisibilityMode,
    /// Use independent per-eye gaze when the headset streams it.
    pub eye_gaze: bool,
    /// Publish the physical right eye on VRFT's left channel and vice versa,
    /// as the reference implementation does after testing in VRChat.
    pub eye_swap_output: bool,
    /// Troubleshooting: negate both horizontal gaze values.
    pub eye_invert_yaw: bool,
    pub eye_offsets: Option<EyeOffsets>,
}

impl Default for QuestProSettings {
    fn default() -> Self {
        Self {
            tongue_smoothing: 55.0,
            tongue_visibility: VisibilityMode::Weighted,
            eye_gaze: true,
            eye_swap_output: true,
            eye_invert_yaw: false,
            eye_offsets: None,
        }
    }
}

impl QuestProSettings {
    fn validated(mut self) -> Result<Self, String> {
        if !self.tongue_smoothing.is_finite() {
            return Err("tongue_smoothing must be a number".into());
        }
        self.tongue_smoothing = self.tongue_smoothing.clamp(0.0, 100.0);
        if let Some(offsets) = self.eye_offsets {
            let values = offsets.left_deg.iter().chain(offsets.right_deg.iter());
            if values
                .clone()
                .any(|value| !value.is_finite() || value.abs() > 30.0)
            {
                return Err("eye offsets must be finite and within 30 degrees".into());
            }
        }
        Ok(self)
    }
}

#[derive(Clone)]
pub struct SettingsStore {
    path: PathBuf,
    inner: Arc<RwLock<QuestProSettings>>,
}

impl SettingsStore {
    pub fn load(root: &Path) -> Self {
        let path = root.join(FILE);
        let settings = match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<QuestProSettings>(&bytes)
                .map_err(|error| error.to_string())
                .and_then(QuestProSettings::validated)
            {
                Ok(settings) => settings,
                Err(error) => {
                    log::warn!(
                        "Quest Pro settings in {} are invalid ({error}); using defaults",
                        path.display()
                    );
                    QuestProSettings::default()
                }
            },
            Err(_) => QuestProSettings::default(),
        };
        Self {
            path,
            inner: Arc::new(RwLock::new(settings)),
        }
    }

    pub fn get(&self) -> QuestProSettings {
        self.inner.read().unwrap().clone()
    }

    /// Merges the given JSON object into the current settings and saves them.
    pub fn merge(&self, patch: &Value) -> Result<QuestProSettings, String> {
        let Value::Object(fields) = patch else {
            return Err("settings update must be a JSON object".into());
        };
        let mut current = self.inner.write().unwrap();
        let mut merged = serde_json::to_value(&*current).map_err(|error| error.to_string())?;
        let object = merged
            .as_object_mut()
            .expect("settings serialize as an object");
        for (key, value) in fields {
            if !object.contains_key(key) {
                return Err(format!("unknown setting {key}"));
            }
            object.insert(key.clone(), value.clone());
        }
        let updated = serde_json::from_value::<QuestProSettings>(merged)
            .map_err(|error| error.to_string())?
            .validated()?;
        self.save(&updated)?;
        *current = updated.clone();
        Ok(updated)
    }

    fn save(&self, settings: &QuestProSettings) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let temporary = self.path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(settings).map_err(|error| error.to_string())?;
        fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
        fs::rename(&temporary, &self.path).map_err(|error| error.to_string())
    }

    #[cfg(test)]
    pub fn in_memory(settings: QuestProSettings) -> Self {
        Self {
            path: PathBuf::from("unused-test-settings.json"),
            inner: Arc::new(RwLock::new(settings)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temporary_root() -> PathBuf {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.local/tongue-tests-rust")
            .join(format!(
                "settings-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn merge_saves_and_reloads_partial_updates() {
        let root = temporary_root();
        let store = SettingsStore::load(&root);
        assert_eq!(store.get(), QuestProSettings::default());
        let updated = store
            .merge(&json!({"tongue_smoothing": 20, "tongue_visibility": "agreement"}))
            .unwrap();
        assert_eq!(updated.tongue_smoothing, 20.0);
        assert_eq!(updated.tongue_visibility, VisibilityMode::Agreement);
        assert!(
            updated.eye_swap_output,
            "untouched fields keep their values"
        );
        assert_eq!(SettingsStore::load(&root).get(), updated);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn merge_rejects_unknown_keys_and_clamps_smoothing() {
        let store = SettingsStore::in_memory(QuestProSettings::default());
        assert!(store.merge(&json!({"not_a_setting": 1})).is_err());
        assert!(store
            .merge(&json!({"tongue_visibility": "sometimes"}))
            .is_err());
        let root = temporary_root();
        let store = SettingsStore::load(&root);
        assert_eq!(
            store
                .merge(&json!({"tongue_smoothing": 250}))
                .unwrap()
                .tongue_smoothing,
            100.0
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults() {
        let root = temporary_root();
        fs::create_dir_all(root.join(".local")).unwrap();
        fs::write(root.join(FILE), b"{not json").unwrap();
        assert_eq!(
            SettingsStore::load(&root).get(),
            QuestProSettings::default()
        );
        fs::remove_dir_all(root).unwrap();
    }
}
