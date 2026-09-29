//! Tracking modules for the local API: what's installed, the module registry,
//! installs, updates and removals, and switching the module in use. Downloads
//! run on their own threads, so every request answers straight away and the
//! app follows progress through `/modules`.
use crate::config_file;
use log::{info, warn};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use vrft_daemon::module_registry::{self, Placed};
use vrft_daemon::plugin_loader::{self, PluginKind};
use vrft_protocol::{
    ConfigPatch, InstalledModule, ModuleOperation, Modules, OperationState, RegistryModule,
    RegistryState,
};

/// How long a fetched registry is used before `/modules` fetches it again.
const REGISTRY_FRESH_FOR: Duration = Duration::from_secs(60 * 60);

/// A request for the tracking loop to load another module, by key. Only the
/// latest request counts.
#[derive(Clone, Default)]
pub struct ModuleSwitch(Arc<Mutex<Option<String>>>);

impl ModuleSwitch {
    pub fn request(&self, key: String) {
        *self.0.lock().unwrap() = Some(key);
    }

    /// The module to switch to, if one was asked for since the last call.
    pub fn take(&self) -> Option<String> {
        self.0.lock().unwrap().take()
    }
}

#[derive(Clone)]
pub struct ModuleManager {
    plugins: PathBuf,
    config: PathBuf,
    /// The host managed modules run in, when it's installed.
    dotnet_host: Option<PathBuf>,
    /// Reaches the tracking loop, which doesn't run in extensions-only mode.
    switch: Option<ModuleSwitch>,
    agent: ureq::Agent,
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    registry: RegistryState,
    operations: Vec<ModuleOperation>,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs())
        .unwrap_or_default()
}

impl ModuleManager {
    pub fn new(
        plugins: PathBuf,
        config: PathBuf,
        registry_url: String,
        dotnet_host: Option<PathBuf>,
        switch: Option<ModuleSwitch>,
    ) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_global(Some(Duration::from_secs(300)))
            .user_agent(concat!("VRFT/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self {
            plugins,
            config,
            dotnet_host,
            switch,
            agent,
            inner: Arc::new(Mutex::new(Inner {
                registry: RegistryState {
                    url: registry_url,
                    ..RegistryState::default()
                },
                operations: Vec::new(),
            })),
        }
    }

    /// Whether the host managed modules run in is installed.
    pub fn dotnet_host(&self) -> bool {
        self.dotnet_host.is_some()
    }

    /// Everything `/modules` shows. Fetches the registry in the background
    /// when it's missing or old.
    pub fn view(&self) -> Modules {
        let stale = {
            let inner = self.inner.lock().unwrap();
            !inner.registry.loading
                && inner
                    .registry
                    .fetched_unix
                    .is_none_or(|at| now_unix().saturating_sub(at) > REGISTRY_FRESH_FOR.as_secs())
                && inner.registry.error.is_none()
        };
        if stale {
            self.refresh();
        }
        let discovered = plugin_loader::discover_plugins(&self.plugins);
        let active = config_file::read(&self.config)
            .map(|config| config_file::active_key(&config.module.active, &discovered))
            .unwrap_or_default();
        let inner = self.inner.lock().unwrap();
        let registry_root = module_registry::registry_dir(&self.plugins);
        let mut installed: Vec<InstalledModule> = discovered
            .into_iter()
            .map(|plugin| {
                let name = config_file::plugin_name(&plugin);
                let removable = plugin.path.starts_with(&registry_root);
                let mut module = InstalledModule {
                    file: plugin.key.clone(),
                    name,
                    runtime: config_file::runtime_name(plugin.kind).into(),
                    removable,
                    ..InstalledModule::default()
                };
                if let Some(manifest) = plugin.manifest {
                    let listed = inner
                        .registry
                        .modules
                        .iter()
                        .find(|entry| entry.module_id == manifest.module_id);
                    module.update = listed
                        .filter(|entry| entry.version != manifest.version)
                        .map(|entry| entry.version.clone());
                    module.pending_restart =
                        module_registry::pending(&self.plugins, &manifest.module_id);
                    let text = |value: String| Some(value).filter(|value| !value.is_empty());
                    module.version = text(manifest.version);
                    module.author = text(manifest.author_name);
                    module.description = text(manifest.module_description);
                    module.usage = text(manifest.usage_instructions);
                    module.page_url = text(manifest.module_page_url);
                    module.module_id = Some(manifest.module_id);
                }
                module
            })
            .collect();
        installed.sort_by_key(|module| module.name.to_lowercase());
        Modules {
            active,
            dotnet_host: self.dotnet_host.is_some(),
            installed,
            registry: inner.registry.clone(),
            operations: inner.operations.clone(),
        }
    }

    /// Fetches the registry again in the background.
    pub fn refresh(&self) {
        let url = {
            let mut inner = self.inner.lock().unwrap();
            if inner.registry.loading {
                return;
            }
            inner.registry.loading = true;
            inner.registry.url.clone()
        };
        let this = self.clone();
        std::thread::spawn(move || {
            let result = module_registry::fetch(&this.agent, &url);
            let mut inner = this.inner.lock().unwrap();
            inner.registry.loading = false;
            match result {
                Ok(modules) => {
                    info!("Module registry: {} modules from {url}", modules.len());
                    inner.registry.modules = modules;
                    inner.registry.error = None;
                    inner.registry.fetched_unix = Some(now_unix());
                }
                Err(error) => {
                    warn!("Module registry fetch failed: {error:#}");
                    inner.registry.error = Some(format!("{error:#}"));
                }
            }
        });
    }

    /// Installs or updates registry module `id` in the background.
    pub fn install(&self, id: &str) -> Result<(), String> {
        let entry = {
            let mut inner = self.inner.lock().unwrap();
            let entry = inner
                .registry
                .modules
                .iter()
                .find(|entry| entry.module_id == id)
                .cloned()
                .ok_or_else(|| {
                    "That module isn't in the registry. Refresh the list and try again.".to_string()
                })?;
            if inner
                .operations
                .iter()
                .any(|operation| operation.module_id == id && operation.state.running())
            {
                return Err(format!("{} is already being installed.", entry.module_name));
            }
            inner
                .operations
                .retain(|operation| operation.module_id != id);
            inner.operations.push(ModuleOperation {
                module_id: id.to_string(),
                name: entry.module_name.clone(),
                state: OperationState::Downloading,
                fraction: None,
                message: "Downloading".into(),
            });
            entry
        };
        let this = self.clone();
        std::thread::spawn(move || {
            let result = this.run_install(&entry);
            let (state, message) = match result {
                Ok(message) => (OperationState::Done, message),
                Err(error) => {
                    warn!("Installing module {} failed: {error:#}", entry.module_name);
                    (OperationState::Failed, format!("{error:#}"))
                }
            };
            this.update_operation(&entry.module_id, |operation| {
                operation.state = state;
                operation.message = message;
            });
        });
        Ok(())
    }

    fn run_install(&self, entry: &RegistryModule) -> anyhow::Result<String> {
        info!(
            "Installing module {} {} from {}",
            entry.module_name, entry.version, entry.download_url
        );
        let bytes = module_registry::download(&self.agent, &entry.download_url, |fraction| {
            self.update_operation(&entry.module_id, |operation| {
                operation.fraction = Some(fraction)
            });
        })?;
        self.update_operation(&entry.module_id, |operation| {
            operation.state = OperationState::Installing;
            operation.message = "Installing".into();
        });
        let staged = module_registry::stage(&self.plugins, entry, &bytes)?;
        let placed = module_registry::place(&self.plugins, &entry.module_id, &staged)?;
        info!(
            "Module {} {} installed ({placed:?})",
            entry.module_name, entry.version
        );
        Ok(match placed {
            Placed::Installed => format!("Installed version {}.", entry.version),
            Placed::AfterRestart => format!(
                "Version {} is downloaded. Restart VRFaceTracking to finish updating, as the module is in use.",
                entry.version
            ),
        })
    }

    fn update_operation(&self, id: &str, change: impl FnOnce(&mut ModuleOperation)) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(operation) = inner
            .operations
            .iter_mut()
            .find(|operation| operation.module_id == id)
        {
            change(operation);
        }
    }

    /// Makes the module with key `file` the tracking module: saves it in
    /// `config.json`, and has the tracking loop load it in place of the one
    /// running. Returns the key saved.
    pub fn use_module(&self, file: &str) -> Result<String, String> {
        let discovered = plugin_loader::discover_plugins(&self.plugins);
        let plugin = plugin_loader::find_plugin(&discovered, file)
            .ok_or_else(|| format!("There's no tracking module called {file} in plugins."))?;
        if plugin.kind == PluginKind::Managed && self.dotnet_host.is_none() {
            return Err(format!(
                "{} is a VRCFT module, which needs {} to run, and that isn't installed. \
                 Reinstall VRFaceTracking to get it.",
                config_file::plugin_name(plugin),
                plugin_loader::DOTNET_HOST
            ));
        }
        let key = plugin.key.clone();
        config_file::apply(
            &self.config,
            &ConfigPatch {
                module: Some(key.clone()),
                ..ConfigPatch::default()
            },
            &discovered,
        )?;
        match &self.switch {
            Some(switch) => {
                info!("Switching the tracking module to {key}");
                switch.request(key.clone());
            }
            None => info!(
                "Tracking module set to {key}; it loads when VRFaceTracking next runs tracking"
            ),
        }
        Ok(key)
    }

    /// Removes registry module `id`, unless it's the one in use.
    pub fn uninstall(&self, id: &str) -> Result<(), String> {
        let active = config_file::read(&self.config)?.module.active;
        let dir = module_registry::module_dir(&self.plugins, id);
        let discovered = plugin_loader::discover_plugins(&self.plugins);
        let in_use = plugin_loader::find_plugin(&discovered, &active)
            .is_some_and(|plugin| plugin.path.starts_with(&dir));
        if in_use {
            return Err(
                "It's the module VRFaceTracking uses. Choose another in Settings first, then remove it."
                    .into(),
            );
        }
        module_registry::uninstall(&self.plugins, id).map_err(|error| format!("{error:#}"))?;
        info!("Removed module {id}");
        self.inner
            .lock()
            .unwrap()
            .operations
            .retain(|operation| operation.module_id != id);
        Ok(())
    }
}
