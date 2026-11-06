use axum::{
    extract::{RawQuery, State},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};
use std::net::IpAddr;
use std::sync::{Arc, RwLock};
use vrft_common::UnifiedTrackingData;

#[derive(Clone)]
struct VRChatState {
    data: Arc<RwLock<UnifiedTrackingData>>,
    osc_ip: IpAddr,
    osc_port: u16,
}

/// `osc_ip` and `osc_port` are where VRChat sends its output back to.
pub fn get_router(data: Arc<RwLock<UnifiedTrackingData>>, osc_ip: IpAddr, osc_port: u16) -> Router {
    let state = VRChatState {
        data,
        osc_ip,
        osc_port,
    };

    Router::new()
        .route("/", get(root_handler))
        .route("/avatar/parameters", get(avatar_parameters_handler))
        .with_state(state)
}

/// `/?HOST_INFO` says where OSC goes; plain `/` is the address tree.
async fn root_handler(State(state): State<VRChatState>, RawQuery(query): RawQuery) -> Json<Value> {
    Json(if query.as_deref() == Some("HOST_INFO") {
        host_info(state.osc_ip, state.osc_port)
    } else {
        address_tree()
    })
}

fn host_info(osc_ip: IpAddr, osc_port: u16) -> Value {
    json!({
        "NAME": "VRFT",
        "OSC_IP": osc_ip.to_string(),
        "OSC_PORT": osc_port,
        "OSC_TRANSPORT": "UDP",
        "EXTENSIONS": {
            "ACCESS": true,
            "CLIPMODE": false,
            "RANGE": true,
            "TYPE": true,
            "VALUE": true
        }
    })
}

/// What we want from VRChat. It sends an app the output matching the paths in
/// its tree, so avatar parameters stay out: we'd only have to discard them.
fn address_tree() -> Value {
    json!({
        "DESCRIPTION": "root node",
        "FULL_PATH": "/",
        "ACCESS": 0,
        "CONTENTS": {
            "avatar": {
                "FULL_PATH": "/avatar",
                "ACCESS": 0,
                "CONTENTS": {
                    "change": {
                        "FULL_PATH": "/avatar/change",
                        "ACCESS": 2,
                        "TYPE": "s"
                    },
                    "avatarfullfacetracking": {
                        "FULL_PATH": "/avatar/avatarfullfacetracking",
                        "ACCESS": 2,
                        "TYPE": "T"
                    }
                }
            }
        }
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_info_says_where_vrchat_should_send() {
        let info = host_info(IpAddr::from([127, 0, 0, 1]), 54321);
        assert_eq!(info["OSC_IP"], "127.0.0.1");
        assert_eq!(info["OSC_PORT"], 54321);
        assert_eq!(info["OSC_TRANSPORT"], "UDP");
    }

    #[test]
    fn the_tree_asks_for_avatar_changes_and_not_parameters() {
        let tree = address_tree();
        let avatar = &tree["CONTENTS"]["avatar"]["CONTENTS"];
        assert_eq!(avatar["change"]["FULL_PATH"], "/avatar/change");
        assert!(avatar.get("parameters").is_none());
    }
}
