use anyhow::Result;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

// Toggle to enable/disable auto-configuration
const SETUP_PICO_ENABLED: bool = false;

pub fn setup_pico_connect(logger: &api::ModuleLogger) {
    if !SETUP_PICO_ENABLED {
        return;
    }

    if let Err(e) = try_setup_pico_connect(logger) {
        logger.warn(&format!("Failed to configure Pico Connect: {}", e));
    }
}

fn try_setup_pico_connect(logger: &api::ModuleLogger) -> Result<()> {
    let appdata = std::env::var("APPDATA")?;
    let settings_path = PathBuf::from(appdata)
        .join("PICO Connect")
        .join("settings.json");

    if !settings_path.exists() {
        // Not finding the file is not an error, just means Pico Connect might not be installed or run yet.
        return Ok(());
    }

    let content = fs::read_to_string(&settings_path)?;
    let mut json: Value = serde_json::from_str(&content)?;

    // We expect a "lab" object in the root
    if let Some(lab) = json.get_mut("lab") {
        if let Some(lab_obj) = lab.as_object_mut() {
            let current_proto = lab_obj
                .get("faceTrackingTransferProtocol")
                .and_then(|v| v.as_i64());
            let current_mode = lab_obj.get("faceTrackingMode").and_then(|v| v.as_i64());

            // Check if updates are needed
            if current_proto == Some(2) && current_mode == Some(1) {
                return Ok(());
            }

            logger.info("Detected incorrect Pico Connect settings. Applying fixes...");

            // Set Legacy Protocol (UDP)
            if current_proto != Some(2) {
                lab_obj.insert(
                    "faceTrackingTransferProtocol".to_string(),
                    serde_json::json!(2),
                );
                logger.info("Set faceTrackingTransferProtocol to 2 (Legacy UDP)");
            }

            // Set Image-Driven Mode
            if current_mode != Some(1) {
                lab_obj.insert("faceTrackingMode".to_string(), serde_json::json!(1));
                logger.info("Set faceTrackingMode to 1 (Image Driven)");
            }

            let new_content = serde_json::to_string_pretty(&json)?;
            fs::write(&settings_path, new_content)?;

            logger.info(
                "Pico Connect settings updated. Please restart Pico Connect if it is running.",
            );
        }
    } else {
        logger.warn("Pico Connect settings 'lab' section not found. Skipping auto-config.");
    }

    Ok(())
}
