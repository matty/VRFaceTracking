use crate::CalibrationData;
use anyhow::{Context, Result};
use log::{error, info};
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

pub struct CalibrationManager {
    pub current_profile_id: String,
    pub data: CalibrationData,
    storage_dir: PathBuf,
}

impl CalibrationManager {
    pub fn new(storage_dir: PathBuf) -> Self {
        Self {
            current_profile_id: "default".to_string(),
            data: CalibrationData::default(),
            storage_dir,
        }
    }

    fn get_profile_path(&self, profile_id: &str) -> PathBuf {
        let filename = if profile_id == "default" {
            "calibration_default.json".to_string()
        } else {
            format!("calibration_{}.json", profile_id)
        };
        self.storage_dir.join(filename)
    }

    fn sanitized_for_save(&self) -> CalibrationData {
        let mut data = self.data.clone();

        for shape in &mut data.shapes {
            if !shape.max.is_finite() {
                shape.max = 0.0;
            }
            if !shape.progress.is_finite() {
                shape.progress = 0.0;
            }
        }

        data
    }

    pub fn save_current_profile(&self) -> Result<()> {
        let path = self.get_profile_path(&self.current_profile_id);
        if let Some(parent) = path.parent() {
            if parent.as_os_str().len() > 0 {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create calibration dir: {:?}", parent))?;
            }
        }
        let file = File::create(&path).context("Failed to create calibration file")?;
        let sanitized = self.sanitized_for_save();
        serde_json::to_writer_pretty(file, &sanitized)
            .context("Failed to serialize calibration data")?;
        info!(
            "Saved calibration profile '{}' to {:?}",
            self.current_profile_id, path
        );
        Ok(())
    }

    pub fn load_profile(&mut self, profile_id: &str) -> Result<()> {
        let path = self.get_profile_path(profile_id);
        if !path.exists() {
            return Err(anyhow::anyhow!("Profile file not found: {:?}", path));
        }

        let file = File::open(&path).context("Failed to open calibration file")?;
        let reader = BufReader::new(file);
        let data: CalibrationData =
            serde_json::from_reader(reader).context("Failed to deserialize calibration data")?;

        self.data = data;
        self.current_profile_id = profile_id.to_string();
        info!(
            "Loaded calibration profile '{}' from {:?}",
            profile_id, path
        );
        Ok(())
    }

    pub fn switch_profile(&mut self, new_profile_id: &str, save_current: bool) -> Result<()> {
        if self.current_profile_id == new_profile_id {
            return Ok(());
        }

        if save_current {
            if let Err(e) = self.save_current_profile() {
                error!(
                    "Failed to save current profile '{}' before switching: {}",
                    self.current_profile_id, e
                );
            }
        }

        match self.load_profile(new_profile_id) {
            Ok(_) => {}
            Err(_) => {
                info!("Profile '{}' not found, creating new.", new_profile_id);
                self.data = CalibrationData::default();
                self.current_profile_id = new_profile_id.to_string();
            }
        }

        Ok(())
    }
}
