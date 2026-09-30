//! The daemon's local HTTP API on 127.0.0.1:27275, which the desktop app
//! uses. The daemon serves `/status`, `/shutdown`, `/config`,
//! `/extensions/enabled` and `/modules/...` itself, and each running
//! extension's routes under `/ext/<id>`.
use crate::config_file;
use crate::daemon_status::DaemonStatus;
use crate::modules::ModuleManager;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use log::{info, warn};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use vrft_daemon::plugin_loader;
use vrft_extension::StatusFn;
use vrft_protocol::{
    routes, Config, ConfigPatch, EnableRequest, EnableResponse, ModuleRequest, Modules,
    ShutdownRequest, Status, UseModuleRequest, PORT,
};

/// What the API serves for one running extension.
pub struct ApiExtension {
    pub id: &'static str,
    pub routes: Option<Router>,
    /// Whether `routes` serve a browser page at `/`.
    pub page: bool,
    pub status: Option<StatusFn>,
}

#[derive(Clone)]
struct ApiState {
    daemon: DaemonStatus,
    running: Arc<AtomicBool>,
    config: PathBuf,
    /// Where tracking modules are found.
    plugins: PathBuf,
    modules: ModuleManager,
    statuses: Arc<Vec<(&'static str, StatusFn)>>,
    /// Where `/` sends a browser: the first extension page.
    home: Option<String>,
}

/// Serves the API on its own thread until the daemon exits.
pub fn start(
    daemon: DaemonStatus,
    running: Arc<AtomicBool>,
    config: PathBuf,
    plugins: PathBuf,
    modules: ModuleManager,
    extensions: Vec<ApiExtension>,
) {
    let router = router(daemon, running, config, plugins, modules, extensions);
    let serve = move || {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("local-api")
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                warn!("Local API runtime failed: {error}");
                return;
            }
        };
        runtime.block_on(async move {
            let address = SocketAddr::from(([127, 0, 0, 1], PORT));
            match tokio::net::TcpListener::bind(address).await {
                Ok(listener) => {
                    info!("Local API: http://127.0.0.1:{PORT}/");
                    if let Err(error) = axum::serve(listener, router).await {
                        warn!("Local API stopped: {error}");
                    }
                }
                Err(error) => warn!("Local API bind failed: {error}"),
            }
        });
    };
    thread::Builder::new()
        .name("local-api-host".into())
        .spawn(serve)
        .expect("couldn't start the local API thread");
}

fn router(
    daemon: DaemonStatus,
    running: Arc<AtomicBool>,
    config: PathBuf,
    plugins: PathBuf,
    modules: ModuleManager,
    extensions: Vec<ApiExtension>,
) -> Router {
    let home = extensions
        .iter()
        .find(|extension| extension.page && extension.routes.is_some())
        .map(|extension| routes::extension(extension.id));
    let mut statuses = Vec::new();
    let mut nested = Vec::new();
    for extension in extensions {
        if let Some(status) = extension.status {
            statuses.push((extension.id, status));
        }
        if let Some(routes) = extension.routes {
            nested.push((extension.id, routes));
        }
    }
    let state = ApiState {
        daemon,
        running,
        config,
        plugins,
        modules,
        statuses: Arc::new(statuses),
        home,
    };
    let mut router = Router::new()
        .route("/", get(index))
        .route(routes::STATUS, get(status))
        .route(routes::SHUTDOWN, post(shutdown))
        .route(routes::EXTENSIONS_ENABLED, post(set_enabled))
        .route(routes::CONFIG, get(get_config).post(patch_config))
        .route(routes::MODULES, get(get_modules))
        .route(routes::MODULES_REFRESH, post(refresh_registry))
        .route(routes::MODULES_INSTALL, post(install_module))
        .route(routes::MODULES_UNINSTALL, post(uninstall_module))
        .route(routes::MODULES_USE, post(use_module))
        .with_state(state);
    for (id, routes) in nested {
        // Nested, an extension's `/` is `/ext/<id>` without the slash.
        let base = routes::extension(id);
        let target = base.clone();
        router = router
            .route(
                &format!("{base}/"),
                get(move || async move { Redirect::permanent(&target) }),
            )
            .nest(&base, routes);
    }
    router
}

async fn index(State(api): State<ApiState>) -> Response {
    match &api.home {
        Some(home) => Redirect::temporary(home).into_response(),
        None => Html(
            "<!doctype html><title>VRFaceTracking</title><p>VRFaceTracking is running. Open the desktop app to see what it's doing.</p>",
        )
        .into_response(),
    }
}

async fn status(State(api): State<ApiState>) -> Json<Status> {
    Json(Status {
        daemon: Some(api.daemon.report()),
        extensions: api
            .statuses
            .iter()
            .map(|(id, status)| (id.to_string(), status()))
            .collect(),
    })
}

async fn shutdown(State(api): State<ApiState>, Json(request): Json<ShutdownRequest>) -> StatusCode {
    let requester = if request.requested_by.is_empty() {
        "a local client"
    } else {
        request.requested_by.as_str()
    };
    info!("Shutdown requested by {requester}");
    api.running.store(false, Ordering::SeqCst);
    StatusCode::ACCEPTED
}

/// Turns an extension on or off in `config.json`. It takes effect when the
/// daemon next starts.
async fn set_enabled(
    State(api): State<ApiState>,
    Json(request): Json<EnableRequest>,
) -> Result<Json<EnableResponse>, (StatusCode, String)> {
    let known = api
        .daemon
        .report()
        .extensions
        .iter()
        .any(|extension| extension.id == request.id);
    if !known {
        return Err((
            StatusCode::NOT_FOUND,
            format!("This VRFaceTracking has no extension called {}", request.id),
        ));
    }
    config_file::write_enabled(&api.config, &request.id, request.enabled)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?;
    info!(
        "Extension {} {} in {}; restart VRFaceTracking to apply",
        request.id,
        if request.enabled {
            "enabled"
        } else {
            "disabled"
        },
        api.config.display()
    );
    Ok(Json(EnableResponse {
        restart_required: true,
    }))
}

async fn get_config(State(api): State<ApiState>) -> Result<Json<Config>, (StatusCode, String)> {
    config_file::view(&api.config, &api.plugins)
        .map(|config| {
            Json(Config {
                dotnet_host: api.modules.dotnet_host(),
                ..config
            })
        })
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))
}

/// Changes settings in `config.json`. They take effect when the daemon next
/// starts.
async fn patch_config(
    State(api): State<ApiState>,
    Json(patch): Json<ConfigPatch>,
) -> Result<Json<Config>, (StatusCode, String)> {
    let modules = plugin_loader::discover_plugins(&api.plugins);
    config_file::apply(&api.config, &patch, &modules)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    info!(
        "Settings changed in {}: {patch:?}; restart VRFaceTracking to apply",
        api.config.display()
    );
    get_config(State(api)).await
}

async fn get_modules(State(api): State<ApiState>) -> Json<Modules> {
    Json(api.modules.view())
}

/// Takes a JSON body, as every change does, so a web page can't post it.
async fn refresh_registry(
    State(api): State<ApiState>,
    Json(_): Json<serde_json::Value>,
) -> Json<Modules> {
    api.modules.refresh();
    Json(api.modules.view())
}

/// Starts installing or updating a registry module; `/modules` shows how it
/// goes.
async fn install_module(
    State(api): State<ApiState>,
    Json(request): Json<ModuleRequest>,
) -> Result<Json<Modules>, (StatusCode, String)> {
    api.modules
        .install(&request.module_id)
        .map_err(|error| (StatusCode::CONFLICT, error))?;
    Ok(Json(api.modules.view()))
}

async fn uninstall_module(
    State(api): State<ApiState>,
    Json(request): Json<ModuleRequest>,
) -> Result<Json<Modules>, (StatusCode, String)> {
    api.modules
        .uninstall(&request.module_id)
        .map_err(|error| (StatusCode::CONFLICT, error))?;
    Ok(Json(api.modules.view()))
}

/// Makes a module the tracking module, loading it without a restart.
async fn use_module(
    State(api): State<ApiState>,
    Json(request): Json<UseModuleRequest>,
) -> Result<Json<Modules>, (StatusCode, String)> {
    api.modules
        .use_module(&request.file)
        .map_err(|error| (StatusCode::CONFLICT, error))?;
    Ok(Json(api.modules.view()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_status::RunMode;
    use serde_json::{json, Value};
    use vrft_extension::ExtensionReport;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vrft_api_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Serves `router` on a free port for the rest of the test.
    fn serve(router: Router) -> SocketAddr {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(async move {
                    let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                    axum::serve(listener, router).await.unwrap();
                })
        });
        address
    }

    fn fetch(url: String) -> (u16, String) {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .into();
        let mut response = agent.get(url).call().unwrap();
        let status = response.status().as_u16();
        (status, response.body_mut().read_to_string().unwrap())
    }

    #[test]
    fn serves_status_extension_routes_and_the_extension_page() {
        let daemon = DaemonStatus::new(RunMode::Normal);
        daemon.set_extensions(vec![ExtensionReport {
            id: "demo".into(),
            name: "Demo".into(),
            enabled: true,
            error: None,
        }]);
        let routes = Router::new()
            .route("/", get(|| async { "demo page" }))
            .route("/thing", get(|| async { "thing" }));
        let config = temp_dir("serve").join("config.json");
        let plugins = temp_dir("serve-plugins");
        let modules = ModuleManager::new(
            plugins.clone(),
            config.clone(),
            "http://127.0.0.1:1/modules".into(),
            None,
            None,
        );
        let address = serve(router(
            daemon,
            Arc::new(AtomicBool::new(true)),
            config,
            plugins,
            modules,
            vec![ApiExtension {
                id: "demo",
                routes: Some(routes),
                page: true,
                status: Some(Box::new(|| json!({"live": true}))),
            }],
        ));

        let (code, body) = fetch(format!("http://{address}/status"));
        assert_eq!(code, 200);
        let status: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(status["extensions"]["demo"]["live"], true);
        assert_eq!(status["daemon"]["extensions"][0]["id"], "demo");
        assert_eq!(status["daemon"]["mode"], "normal");

        assert_eq!(
            fetch(format!("http://{address}/ext/demo/thing")),
            (200, "thing".into())
        );
        // `/`, the bare extension path and its slash form all reach the page.
        for path in ["/", "/ext/demo", "/ext/demo/"] {
            assert_eq!(
                fetch(format!("http://{address}{path}")),
                (200, "demo page".into()),
                "{path}"
            );
        }
        assert_eq!(fetch(format!("http://{address}/ext/other/thing")).0, 404);
    }
}
