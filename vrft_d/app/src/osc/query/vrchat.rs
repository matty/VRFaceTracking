use axum::{extract::State, routing::get, Json, Router};
use common::UnifiedTrackingData;
use serde_json::{json, Value};
use std::sync::{Arc, RwLock};

#[derive(Clone)]
struct VRChatState {
    data: Arc<RwLock<UnifiedTrackingData>>,
    osc_port: u16,
}

pub fn get_router(data: Arc<RwLock<UnifiedTrackingData>>, osc_port: u16) -> Router {
    let state = VRChatState { data, osc_port };

    Router::new()
        .route("/", get(root_handler))
        .route("/HOST_INFO", get(host_info_handler))
        .route("/avatar/parameters", get(avatar_parameters_handler))
        .with_state(state)
}

async fn root_handler(State(state): State<VRChatState>) -> Json<Value> {
    Json(json!({
        "DESCRIPTION": "VRFaceTracking",
        "OSC_PORT": state.osc_port,
        "OSC_IP": "127.0.0.1"
    }))
}

async fn host_info_handler(State(state): State<VRChatState>) -> Json<Value> {
    Json(json!({
        "name": "VRFaceTracking",
        "osc_port": state.osc_port,
        "osc_ip": "127.0.0.1",
        "extensions": {
            "ACCESS": true,
            "CLIPMODE": false,
            "RANGE": true,
            "TYPE": true,
            "VALUE": true
        }
    }))
}

async fn avatar_parameters_handler(State(state): State<VRChatState>) -> Json<Value> {
    let data = state.data.read().unwrap();

    // Construct OSC Query tree
    Json(json!({
        "DESCRIPTION": "Avatar Parameters",
        "CONTENTS": {
            "v2": {
                "DESCRIPTION": "VRFT v2 Parameters",
                "CONTENTS": {
                    "Eye": {
                        "DESCRIPTION": "Eye Tracking",
                        "CONTENTS": {
                            "Left": {
                                "DESCRIPTION": "Left Eye",
                                "CONTENTS": {
                                    "Openness": {
                                        "DESCRIPTION": "Openness",
                                        "TYPE": "f",
                                        "VALUE": data.eye.left.openness,
                                        "ACCESS": 1
                                    },
                                    "Pupil": {
                                        "DESCRIPTION": "Pupil Diameter",
                                        "TYPE": "f",
                                        "VALUE": data.eye.left.pupil_diameter_mm,
                                        "ACCESS": 1
                                    },
                                    "Gaze": {
                                        "DESCRIPTION": "Gaze",
                                        "CONTENTS": {
                                            "x": { "TYPE": "f", "VALUE": data.eye.left.gaze.x, "ACCESS": 1 },
                                            "y": { "TYPE": "f", "VALUE": data.eye.left.gaze.y, "ACCESS": 1 }
                                        }
                                    }
                                }
                            },
                            "Right": {
                                "DESCRIPTION": "Right Eye",
                                "CONTENTS": {
                                    "Openness": {
                                        "DESCRIPTION": "Openness",
                                        "TYPE": "f",
                                        "VALUE": data.eye.right.openness,
                                        "ACCESS": 1
                                    },
                                    "Pupil": {
                                        "DESCRIPTION": "Pupil Diameter",
                                        "TYPE": "f",
                                        "VALUE": data.eye.right.pupil_diameter_mm,
                                        "ACCESS": 1
                                    },
                                    "Gaze": {
                                        "DESCRIPTION": "Gaze",
                                        "CONTENTS": {
                                            "x": { "TYPE": "f", "VALUE": data.eye.right.gaze.x, "ACCESS": 1 },
                                            "y": { "TYPE": "f", "VALUE": data.eye.right.gaze.y, "ACCESS": 1 }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }))
}
