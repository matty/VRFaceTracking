use crate::CalibrationData;
use anyhow::{Context, Result};
use log::info;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

const CALIBRATION_FILENAME: &str = "calibration.json";

pub struct CalibrationManager {
    pub data: CalibrationData,
    storage_path: PathBuf,
}

impl CalibrationManager {
    pub fn new(storage_dir: PathBuf) -> Self {
        Self {
            data: CalibrationData::default(),
            storage_path: storage_dir.join(CALIBRATION_FILENAME),
        }
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
            if !shape.mean.is_finite() {
                shape.mean = 0.0;
            }
            if !shape.std_dev.is_finite() {
                shape.std_dev = 0.0;
            }
            if !shape.confidence.is_finite() {
                shape.confidence = 0.0;
            }
            if !shape.max_confidence.is_finite() {
                shape.max_confidence = 0.0;
            }
        }

        data
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.storage_path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create calibration dir: {:?}", parent))?;
            }
        }
        let file = File::create(&self.storage_path).context("Failed to create calibration file")?;
        let sanitized = self.sanitized_for_save();
        serde_json::to_writer_pretty(file, &sanitized)
            .context("Failed to serialize calibration data")?;
        info!("Saved calibration data to {:?}", self.storage_path);
        Ok(())
    }

    pub fn load(&mut self) -> Result<()> {
        if !self.storage_path.exists() {
            info!(
                "No calibration file found at {:?}, using defaults",
                self.storage_path
            );
            return Ok(());
        }

        let file = File::open(&self.storage_path).context("Failed to open calibration file")?;
        let reader = BufReader::new(file);
        let data: CalibrationData =
            serde_json::from_reader(reader).context("Failed to deserialize calibration data")?;

        self.data = data;
        info!("Loaded calibration data from {:?}", self.storage_path);
        Ok(())
    }
}
