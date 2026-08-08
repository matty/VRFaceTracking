use axum::{extract::State, Json, Router};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Clone)]
struct ExtensionState {
    debug_state: Arc<RwLock<HashMap<String, f32>>>,
}

pub fn get_router(debug_state: Arc<RwLock<HashMap<String, f32>>>) -> Router {
    let state = ExtensionState { debug_state };

    Router::new()
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
