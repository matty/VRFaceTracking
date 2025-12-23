use std::fs;
use std::path::{Path, PathBuf};

use common::calibration_manager::CalibrationManager;

fn get_test_dir() -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push("vrft_test_calibration_rounding");
    let _ = fs::create_dir_all(&path);
    path
}

fn cleanup_test_dir(path: &Path) {
    let _ = fs::remove_dir_all(path);
}

#[test]
fn test_calibration_save_does_not_round_stats() {
    let dir = get_test_dir();
    cleanup_test_dir(&dir);
    let _ = fs::create_dir_all(&dir);

    let mut manager = CalibrationManager::new(dir.clone());

    manager.data.shapes[0].mean = 1.2345;
    manager.data.shapes[0].std_dev = 0.98765;

    manager.data.shapes[0].calibrated_min = 1.2345;
    manager.data.shapes[0].calibrated_max = 1.2301;

    manager.save_current_profile().expect("save should succeed");

    let path = dir.join("calibration_default.json");
    let contents = fs::read_to_string(&path).expect("file should exist");
    let json: serde_json::Value = serde_json::from_str(&contents).expect("valid json");

    let shapes = json["shapes"].as_array().expect("shapes array");

    let mean0 = shapes[0]["mean"].as_f64().unwrap();
    let std0 = shapes[0]["std_dev"].as_f64().unwrap();
    assert!((mean0 - 1.2345).abs() < 1e-9, "mean should not round, got {}", mean0);
    assert!((std0 - 0.98765).abs() < 1e-9, "std_dev should not round, got {}", std0);

    let min0 = shapes[0]["calibrated_min"].as_f64().unwrap();
    let max0 = shapes[0]["calibrated_max"].as_f64().unwrap();
    assert!(min0 <= max0, "legacy min/max should be sanitized to ordered range");

    cleanup_test_dir(&dir);
}
