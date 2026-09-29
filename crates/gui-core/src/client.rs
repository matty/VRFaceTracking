//! Client for the daemon's local HTTP API, served by `vrft_d` on port 27275.
//!
//! The wire types are `vrft-protocol`'s, which the daemon serves. Each
//! extension's part of the status stays JSON here; the extension's own
//! protocol crate gives it a type.
use anyhow::{bail, Context as _, Result};
use rust_i18n::t;
use std::time::Duration;
use vrft_protocol::{routes, EnableRequest, ModuleRequest, ShutdownRequest, UseModuleRequest};
pub use vrft_protocol::{
    AdjustmentGroup, AdjustmentTuning, Config, ConfigPatch, CorrectorsTuning, DaemonReport,
    ExtensionReport, FilterTuning, InstalledModule, ModuleOperation, ModuleStatus, Modules,
    OperationState, OutputTarget, Plugin, RegistryModule, RegistryState, RunMode, Status, Tuning,
    VrchatLink, DEFAULT_ADDRESS,
};

/// Longest the app waits for one response. The daemon is on this machine, so
/// anything slower means it is stuck or gone.
const TIMEOUT: Duration = Duration::from_millis(800);

pub struct DaemonClient {
    agent: ureq::Agent,
    address: String,
}

impl DaemonClient {
    pub fn new(address: impl Into<String>) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .http_status_as_error(false)
            .build()
            .into();
        Self {
            agent,
            address: address.into(),
        }
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    /// The agent requests go through, for an extension's own calls.
    pub fn agent(&self) -> &ureq::Agent {
        &self.agent
    }

    /// The URL of `path` on the daemon, such as `/ext/quest-pro/frame`.
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
    }

    pub fn status(&self) -> Result<Status> {
        let mut response = self
            .agent
            .get(self.url(routes::STATUS))
            .call()
            .with_context(|| self.unreachable())?;
        if !response.status().is_success() {
            bail!("{}", t!("client.answered", status = response.status()));
        }
        response
            .body_mut()
            .read_json()
            .context(t!("client.bad_status"))
    }

    /// Asks the daemon to shut down, as Ctrl-C in its console would.
    pub fn shutdown(&self) -> Result<()> {
        let response = self
            .agent
            .post(self.url(routes::SHUTDOWN))
            .send_json(ShutdownRequest {
                requested_by: "the desktop app".into(),
            })
            .with_context(|| self.unreachable())?;
        match response.status().as_u16() {
            200..=299 => Ok(()),
            404 => {
                bail!("{}", t!("client.cant_stop"))
            }
            status => bail!("{}", t!("client.answered", status = status)),
        }
    }

    /// Turns an extension on or off in the daemon's `config.json`. It takes
    /// effect when the daemon next starts.
    pub fn set_extension_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let mut response = self
            .agent
            .post(self.url(routes::EXTENSIONS_ENABLED))
            .send_json(EnableRequest {
                id: id.into(),
                enabled,
            })
            .with_context(|| self.unreachable())?;
        if response.status().is_success() {
            return Ok(());
        }
        let reason = response.body_mut().read_to_string().unwrap_or_default();
        match response.status().as_u16() {
            // A daemon from before extensions has no such route.
            404 if reason.is_empty() => {
                bail!("{}", t!("client.cant_switch_extensions"))
            }
            status if reason.is_empty() => bail!("{}", t!("client.answered", status = status)),
            _ => bail!(reason),
        }
    }

    /// The settings in `config.json` people change.
    pub fn config(&self) -> Result<Config> {
        self.get(routes::CONFIG, &t!("client.cant_change_settings"))
    }

    /// Changes some settings in `config.json`, from VRFT's next start.
    pub fn update_config(&self, patch: &ConfigPatch) -> Result<Config> {
        self.post(routes::CONFIG, Some(patch))
    }

    /// Installed tracking modules, the module registry, and installs in
    /// progress.
    pub fn modules(&self) -> Result<Modules> {
        self.get(routes::MODULES, &t!("client.cant_install_modules"))
    }

    /// Fetches the module registry again.
    pub fn refresh_registry(&self) -> Result<Modules> {
        self.post(routes::MODULES_REFRESH, Some(&serde_json::json!({})))
    }

    /// Starts installing, or updating, registry module `id`.
    pub fn install_module(&self, id: &str) -> Result<Modules> {
        self.post(
            routes::MODULES_INSTALL,
            Some(&ModuleRequest {
                module_id: id.into(),
            }),
        )
    }

    /// Removes registry module `id`.
    pub fn uninstall_module(&self, id: &str) -> Result<Modules> {
        self.post(
            routes::MODULES_UNINSTALL,
            Some(&ModuleRequest {
                module_id: id.into(),
            }),
        )
    }

    /// Makes the module with key `file` the tracking module. VRFT loads it
    /// straight away; `/status` shows how that goes.
    pub fn use_module(&self, file: &str) -> Result<Modules> {
        self.post(
            routes::MODULES_USE,
            Some(&UseModuleRequest { file: file.into() }),
        )
    }

    /// GETs `path` as JSON. `missing` explains a 404, such as an extension
    /// that is turned off.
    pub fn get<T: serde::de::DeserializeOwned>(&self, path: &str, missing: &str) -> Result<T> {
        let mut response = self
            .agent
            .get(self.url(path))
            .call()
            .with_context(|| self.unreachable())?;
        match response.status().as_u16() {
            200..=299 => {}
            404 => bail!("{missing}"),
            status => bail!("{}", t!("client.answered", status = status)),
        }
        response
            .body_mut()
            .read_json()
            .context(t!("client.bad_reply"))
    }

    /// POSTs `body`, or nothing, to `path` and reads the JSON reply.
    pub fn post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: Option<&impl serde::Serialize>,
    ) -> Result<T> {
        let request = self.agent.post(self.url(path));
        let mut response = match body {
            Some(body) => request.send_json(body),
            None => request.send_empty(),
        }
        .with_context(|| self.unreachable())?;
        if !response.status().is_success() {
            // The daemon explains refusals in plain text.
            let reason = response.body_mut().read_to_string().unwrap_or_default();
            if reason.is_empty() {
                bail!("{}", t!("client.answered", status = response.status()));
            }
            bail!(reason);
        }
        response
            .body_mut()
            .read_json()
            .context(t!("client.bad_reply"))
    }

    /// What an error says when the daemon can't be reached at all.
    pub fn unreachable(&self) -> String {
        t!("client.unreachable", address = self.address).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_join_the_address_and_a_route() {
        let client = DaemonClient::new("127.0.0.1:1");
        assert_eq!(client.url(routes::STATUS), "http://127.0.0.1:1/status");
        assert_eq!(
            client.url(&format!("{}/frame", routes::extension("quest-pro"))),
            "http://127.0.0.1:1/ext/quest-pro/frame"
        );
    }
}
