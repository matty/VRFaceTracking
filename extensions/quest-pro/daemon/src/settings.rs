//! User-adjustable Quest Pro tongue and eye settings, saved in `.local/`.
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

const FILE: &str = ".local/quest-pro-settings.json";

pub use vrft_quest_pro_protocol::{
    EyeOffsets, Settings as QuestProSettings, SettingsPatch, VisibilityMode,
};

/// Settings that are safe to use, with smoothing clamped to its range.
fn validated(mut settings: QuestProSettings) -> Result<QuestProSettings, String> {
    for (name, smoothing) in [
        ("tongue_smoothing", &mut settings.tongue_smoothing),
        ("pupil_smoothing", &mut settings.pupil_smoothing),
    ] {
        if !smoothing.is_finite() {
            return Err(format!("{name} must be a number"));
        }
        *smoothing = smoothing.clamp(0.0, 100.0);
    }
    if let Some(offsets) = settings.eye_offsets {
        let values = offsets.left_deg.iter().chain(offsets.right_deg.iter());
        if values
            .clone()
            .any(|value| !value.is_finite() || value.abs() > 30.0)
        {
            return Err("eye offsets must be finite and within 30 degrees".into());
        }
    }
    Ok(settings)
}

/// Settings saved before `mouth_model` existed (every save writes every
/// field) have `weighted` because it was the default then, not because it
/// was chosen; the cameras deciding is the default now.
fn from_before_mouth_model(mut saved: Value) -> Value {
    if let Value::Object(fields) = &mut saved {
        if !fields.contains_key("mouth_model")
            && fields.get("tongue_visibility") == Some(&Value::from("weighted"))
        {
            fields.insert("tongue_visibility".into(), Value::from("camera"));
        }
    }
    saved
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
            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes)
                .map(from_before_mouth_model)
                .and_then(serde_json::from_value::<QuestProSettings>)
                .map_err(|error| error.to_string())
                .and_then(validated)
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

    /// Applies `patch` to the current settings and saves them.
    pub fn apply(&self, patch: &SettingsPatch) -> Result<QuestProSettings, String> {
        self.merge(&serde_json::to_value(patch).map_err(|error| error.to_string())?)
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
        let updated = validated(
            serde_json::from_value::<QuestProSettings>(merged)
                .map_err(|error| error.to_string())?,
        )?;
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
            .join("../../../.local/tongue-tests-rust")
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
        assert_eq!(
            store
                .merge(&json!({"pupil_smoothing": -5}))
                .unwrap()
                .pupil_smoothing,
            0.0
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_old_default_moves_to_the_cameras_but_a_choice_stays() {
        let root = temporary_root();
        fs::create_dir_all(root.join(".local")).unwrap();
        fs::write(
            root.join(FILE),
            br#"{"tongue_smoothing": 47, "tongue_visibility": "weighted"}"#,
        )
        .unwrap();
        let store = SettingsStore::load(&root);
        assert_eq!(store.get().tongue_visibility, VisibilityMode::Camera);
        assert_eq!(store.get().tongue_smoothing, 47.0);
        // Saved now, with mouth_model, a choice of weighted stays.
        store
            .merge(&json!({"tongue_visibility": "weighted"}))
            .unwrap();
        assert_eq!(
            SettingsStore::load(&root).get().tongue_visibility,
            VisibilityMode::Weighted
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
