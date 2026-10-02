//! The daemon's local HTTP API on 127.0.0.1:27275, which the desktop app
//! uses. The daemon serves `/status`, `/shutdown`, `/config`,
//! `/extensions/enabled` and `/modules/...` itself, and each running
//! extension's routes under `/ext/<id>`.
use crate::config_file;
use crate::daemon_status::DaemonStatus;
use crate::modules::ModuleManager;
use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use log::{info, warn};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
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
        let runtime = match tokio::runtime::Builder::new_current_thread()
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
    router.layer(middleware::from_fn(local_host_only))
}

/// Refuses a request whose `Host` isn't this PC's loopback address. A web
/// page can point its own name at 127.0.0.1 (DNS rebinding) to reach the API
/// from the browser, but its requests still carry that name. Browsers always
/// send `Host`, so a request without one isn't from a page and is let
/// through.
async fn local_host_only(request: Request, next: Next) -> Response {
    if let Some(host) = request.headers().get(header::HOST) {
        if !host.to_str().is_ok_and(local_host) {
            return (
                StatusCode::FORBIDDEN,
                "VRFaceTracking only answers at 127.0.0.1 or localhost.",
            )
                .into_response();
        }
    }
    next.run(request).await
}

/// Whether `host`, as a `Host` header says it, is `127.0.0.1` or
/// `localhost`, with any port.
fn local_host(host: &str) -> bool {
    let name = match host.rsplit_once(':') {
        Some((name, port)) if port.parse::<u16>().is_ok() => name,
        _ => host,
    };
    name == "127.0.0.1" || name.eq_ignore_ascii_case("localhost")
}

/// Runs `work`, which reads files or waits for the config lock, off the
/// thread that answers requests.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|error| std::panic::resume_unwind(error.into_panic()))
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
    let (config, id) = (api.config.clone(), request.id.clone());
    blocking(move || config_file::write_enabled(&config, &id, request.enabled))
        .await
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
    let dotnet_host = api.modules.dotnet_host();
    blocking(move || config_file::view(&api.config, &api.plugins))
        .await
        .map(|config| {
            Json(Config {
                dotnet_host,
                ..config
            })
        })
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))
}

/// Changes settings in `config.json`. A new module loads straight away; the
/// rest take effect when the daemon next starts.
async fn patch_config(
    State(api): State<ApiState>,
    Json(patch): Json<ConfigPatch>,
) -> Result<Json<Config>, (StatusCode, String)> {
    let modules = api.modules.clone();
    let patch = blocking(move || modules.apply_config(&patch).map(|()| patch))
        .await
        .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    info!(
        "Settings changed in {}: {patch:?}; restart VRFaceTracking to apply",
        api.config.display()
    );
    get_config(State(api)).await
}

async fn get_modules(State(api): State<ApiState>) -> Json<Modules> {
    Json(blocking(move || api.modules.view()).await)
}

/// Takes a JSON body, as every change does, so a web page can't post it.
async fn refresh_registry(
    State(api): State<ApiState>,
    Json(_): Json<serde_json::Value>,
) -> Json<Modules> {
    api.modules.refresh();
    get_modules(State(api)).await
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
    Ok(get_modules(State(api)).await)
}

async fn uninstall_module(
    State(api): State<ApiState>,
    Json(request): Json<ModuleRequest>,
) -> Result<Json<Modules>, (StatusCode, String)> {
    let modules = api.modules.clone();
    blocking(move || modules.uninstall(&request.module_id))
        .await
        .map_err(|error| (StatusCode::CONFLICT, error))?;
    Ok(get_modules(State(api)).await)
}

/// Makes a module the tracking module, loading it without a restart.
async fn use_module(
    State(api): State<ApiState>,
    Json(request): Json<UseModuleRequest>,
) -> Result<Json<Modules>, (StatusCode, String)> {
    let modules = api.modules.clone();
    blocking(move || modules.use_module(&request.file))
        .await
        .map_err(|error| (StatusCode::CONFLICT, error))?;
    Ok(get_modules(State(api)).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_status::RunMode;
    use crate::test_support::temp_dir;
    use serde_json::{json, Value};
    use std::io::{Read as _, Write as _};
    use vrft_extension::ExtensionReport;

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
        let config = temp_dir("api_serve").join("config.json");
        let plugins = temp_dir("api_serve_plugins");
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

        // `localhost` is this PC too, but another name, as a page that
        // points its own at 127.0.0.1 sends, isn't.
        let port = address.port();
        assert_eq!(get_as(address, &format!("localhost:{port}")), 200);
        assert_eq!(get_as(address, "127.0.0.1"), 200);
        assert_eq!(get_as(address, &format!("evil.example:{port}")), 403);
        assert_eq!(get_as(address, "127.0.0.1.evil.example"), 403);
    }

    /// The status code `/status` answers with when asked for as `host`.
    fn get_as(address: SocketAddr, host: &str) -> u16 {
        let mut stream = std::net::TcpStream::connect(address).unwrap();
        write!(
            stream,
            "GET /status HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        reply.split(' ').nth(1).unwrap().parse().unwrap()
    }

    #[test]
    fn hosts_other_than_this_pc_are_refused() {
        assert!(local_host("127.0.0.1:27275"));
        assert!(local_host("LOCALHOST:27275"));
        assert!(local_host("localhost"));
        assert!(!local_host("evil.example:27275"));
        assert!(!local_host("127.0.0.1:27275.evil.example"));
        assert!(!local_host("localhost.evil.example"));
        assert!(!local_host(""));
    }
}
