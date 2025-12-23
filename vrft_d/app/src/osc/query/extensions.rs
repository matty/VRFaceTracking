use axum::{extract::State, routing::get, Json, Router};
use common::CalibrationData;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct CalibrationStatus {
    pub is_calibrating: bool,
    pub progress: f32,
    pub elapsed: f32,
    pub duration: f32,
}

#[derive(Clone)]
struct ExtensionState {
    debug_state: Arc<RwLock<HashMap<String, f32>>>,
    calibration_status: Arc<RwLock<CalibrationStatus>>,
    calibration_data: Arc<RwLock<CalibrationData>>,
    calibration_request: Arc<RwLock<Option<f32>>>,
}

pub fn get_router(
    debug_state: Arc<RwLock<HashMap<String, f32>>>,
    calibration_status: Arc<RwLock<CalibrationStatus>>,
    calibration_data: Arc<RwLock<CalibrationData>>,
    calibration_request: Arc<RwLock<Option<f32>>>,
) -> Router {
    let state = ExtensionState {
        debug_state,
        calibration_status,
        calibration_data,
        calibration_request,
    };

    Router::new()
        .route("/calibration", get(calibration_status_handler))
        .route("/calibration/status", get(calibration_status_handler))
        .route("/calibration/data", get(calibration_data_handler))
        .route(
            "/calibration/start",
            axum::routing::post(start_calibration_handler),
        )
        .route("/debug/params", axum::routing::post(debug_params_handler))
        .with_state(state)
}

async fn debug_params_handler(
    State(state): State<ExtensionState>,
    Json(payload): Json<HashMap<String, f32>>,
) -> Json<Value> {
    let mut debug = state.debug_state.write().unwrap();
    for (k, v) in payload {
        debug.insert(k, v);
    }
    log::info!("Updated debug overrides: {:?}", *debug);
    Json(json!({
        "status": "ok",
        "current_overrides": *debug
    }))
}

async fn calibration_status_handler(State(state): State<ExtensionState>) -> Json<Value> {
    let status = state.calibration_status.read().unwrap().clone();
    Json(json!({
        "status": "ok",
        "calibration": status
    }))
}

async fn calibration_data_handler(State(state): State<ExtensionState>) -> Json<Value> {
    let data = state.calibration_data.read().unwrap().clone();
    Json(json!({
        "status": "ok",
        "data": data
    }))
}

#[derive(Debug, serde::Deserialize)]
struct StartCalibrationPayload {
    duration: Option<f32>,
}

async fn start_calibration_handler(
    State(state): State<ExtensionState>,
    payload: Option<Json<StartCalibrationPayload>>,
) -> Json<Value> {
    let status = state.calibration_status.read().unwrap().clone();
    if status.is_calibrating {
        return Json(json!({
            "status": "already_calibrating",
            "message": "Calibration is already in progress",
            "calibration": status
        }));
    }

    let duration = payload
        .as_ref()
        .and_then(|p| p.duration)
        .unwrap_or(30.0)
        .max(1.0);

    // Signal the consumer thread to start calibration
    if let Ok(mut req) = state.calibration_request.write() {
        *req = Some(duration);
    }

    Json(json!({
        "status": "starting",
        "requested_duration": duration
    }))
}
