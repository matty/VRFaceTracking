//! Reading and changing `config.json` straight from the app, for while VRFT
//! isn't running to do it.
use serde_json::Value;
use std::time::Duration;
use vrft_gui_core::processes::HeldMutex;

/// How long writing `config.json` waits for a change VRFT is saving.
const LOCK_WAIT: Duration = Duration::from_secs(5);

const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// `config.json` as JSON. `Ok(None)` when there isn't one yet.
pub fn read() -> anyhow::Result<Option<Value>> {
    let path = vrft_gui_core::paths::config_file()
        .ok_or_else(|| anyhow::anyhow!("Can't find config.json"))?;
    match std::fs::read(&path) {
        // Notepad may start it with a byte order mark.
        Ok(bytes) => Ok(Some(serde_json::from_slice(
            bytes.strip_prefix(UTF8_BOM).unwrap_or(&bytes),
        )?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Applies `change` to `config.json`, which is created when missing.
pub fn edit(change: impl FnOnce(&mut Value) -> anyhow::Result<()>) -> anyhow::Result<()> {
    let path = vrft_gui_core::paths::config_file()
        .ok_or_else(|| anyhow::anyhow!("Can't find config.json"))?;
    // VRFT may be running without answering, and save a change of its own.
    let _locked = HeldMutex::take(vrft_protocol::CONFIG_LOCK, LOCK_WAIT)
        .ok_or_else(|| anyhow::anyhow!("Another change is still being saved. Try again."))?;
    let mut config = read()?.unwrap_or_else(|| serde_json::json!({}));
    if !config.is_object() {
        anyhow::bail!("{} isn't a JSON object", path.display());
    }
    change(&mut config)?;
    // Not VRFT's own `config.json.pending`, which an older VRFT writes
    // without the lock.
    let pending = path.with_extension("json.app-pending");
    std::fs::write(&pending, serde_json::to_string_pretty(&config)?)?;
    std::fs::rename(&pending, &path)?;
    Ok(())
}

/// Turns an extension on or off.
pub fn set_extension_enabled(id: &str, enabled: bool) -> anyhow::Result<()> {
    edit(|config| {
        vrft_protocol::set_extension_enabled(config, id, enabled).map_err(anyhow::Error::msg)
    })
}

/// Marks the first-launch setup finished, so it doesn't open again.
pub fn set_setup_done() -> anyhow::Result<()> {
    edit(|config| {
        config["setup_done"] = Value::Bool(true);
        Ok(())
    })
}

/// Whether the first-launch setup should open: on a new install, before
/// `config.json` exists or while it chooses no module, until setup is
/// finished or skipped. A config people have already set up, choosing a
/// module themselves, never needs it.
pub fn needs_setup(config: Option<&Value>) -> bool {
    let Some(config) = config else {
        return true;
    };
    !config["setup_done"].as_bool().unwrap_or(false)
        && config["module"]["active"]
            .as_str()
            .is_none_or(|active| active.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn setup_opens_until_a_module_is_chosen_or_it_is_done() {
        assert!(needs_setup(None));
        assert!(needs_setup(Some(&json!({}))));
        assert!(needs_setup(Some(&json!({"module": {"active": " "}}))));
        assert!(!needs_setup(Some(
            &json!({"module": {"active": "vd_module.dll"}})
        )));
        assert!(!needs_setup(Some(&json!({"setup_done": true}))));
    }
}
