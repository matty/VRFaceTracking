//! Reading and changing `config.json` for the desktop app. Changes keep
//! everything else in the file, including settings this VRFT doesn't know,
//! and are checked against the config VRFT reads before they're saved.
use serde_json::{json, Map, Value};
use std::path::Path;
use vrft_common::mutations::{adjustment_group, valid_range, ADJUSTMENT_GROUPS};
use vrft_common::{MutationConfig, MutatorConfig};
use vrft_daemon::plugin_loader::{self, PluginKind};
use vrft_protocol::{
    module_name as friendly_name, AdjustmentGroup, AdjustmentTuning, Config, ConfigPatch,
    CorrectorsTuning, FilterTuning, Plugin, Tuning,
};

/// The output modes the desktop app offers, as `config.json` names them.
/// `Resonite` still works when set in the file by hand, but isn't offered.
const OUTPUT_MODES: [&str; 2] = ["VRChat", "Generic"];

/// The file as JSON, or an empty object when there isn't one yet.
fn read_json(config: &Path) -> Result<Value, String> {
    match std::fs::read(config) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| format!("{} isn't valid JSON: {error}", config.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(error) => Err(format!("Can't read {}: {error}", config.display())),
    }
}

/// Applies `change` to the file's JSON, checks VRFT can still read it, and
/// saves it.
fn edit(
    config: &Path,
    change: impl FnOnce(&mut Map<String, Value>) -> Result<(), String>,
) -> Result<(), String> {
    let mut root = read_json(config)?;
    let Some(object) = root.as_object_mut() else {
        return Err(format!("{} isn't a JSON object", config.display()));
    };
    change(object)?;
    serde_json::from_value::<MutationConfig>(root.clone())
        .map_err(|error| format!("That isn't a setting VRFaceTracking can use: {error}"))?;
    let text = serde_json::to_string_pretty(&root).map_err(|error| error.to_string())?;
    let pending = config.with_extension("json.pending");
    std::fs::write(&pending, text)
        .map_err(|error| format!("Can't write {}: {error}", pending.display()))?;
    std::fs::rename(&pending, config)
        .map_err(|error| format!("Can't replace {}: {error}", config.display()))
}

/// `value` to three decimals, so 0.3 isn't saved as 0.30000001192092896.
fn tidy(value: f32) -> f64 {
    (f64::from(value) * 1000.0).round() / 1000.0
}

/// The object at `key` in `object`, made one if it's missing or isn't.
fn section<'a>(object: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    let value = object.entry(key).or_insert_with(|| json!({}));
    if !value.is_object() {
        *value = json!({});
    }
    value.as_object_mut().expect("just made an object")
}

/// Sets `extensions.<id>.enabled`.
pub fn write_enabled(config: &Path, id: &str, enabled: bool) -> Result<(), String> {
    edit(config, |root| {
        let mut value = Value::Object(std::mem::take(root));
        let result = vrft_protocol::set_extension_enabled(&mut value, id, enabled);
        if let Value::Object(object) = value {
            *root = object;
        }
        result
    })
}

/// The config VRFT reads from the file.
pub fn read(config: &Path) -> Result<MutationConfig, String> {
    serde_json::from_value(read_json(config)?)
        .map_err(|error| format!("{} can't be read: {error}", config.display()))
}

/// What to call a module: its registry name, or else from its file name.
pub fn plugin_name(plugin: &plugin_loader::DiscoveredPlugin) -> String {
    plugin
        .manifest
        .as_ref()
        .map(|manifest| manifest.module_name.trim())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| friendly_name(&plugin.name))
}

/// `native` or `dotnet`, as the API says it.
pub fn runtime_name(kind: PluginKind) -> &'static str {
    match kind {
        PluginKind::Native => "native",
        PluginKind::Managed => "dotnet",
    }
}

/// `module.active` as the key of the module it names, so a config written
/// with a bare file name still matches that module's key. Left as it is
/// when no module matches.
pub fn active_key(active: &str, plugins: &[plugin_loader::DiscoveredPlugin]) -> String {
    plugin_loader::find_plugin(plugins, active)
        .map(|plugin| plugin.key.clone())
        .unwrap_or_else(|| active.to_string())
}

/// The settings people change, and the tracking modules in `plugins`.
pub fn view(config: &Path, plugins: &Path) -> Result<Config, String> {
    let parsed = read(config)?;
    let discovered = plugin_loader::discover_plugins(plugins);
    let mut modules: Vec<Plugin> = discovered
        .iter()
        .map(|plugin| Plugin {
            name: plugin_name(plugin),
            module_id: plugin
                .manifest
                .as_ref()
                .map(|manifest| manifest.module_id.clone()),
            file: plugin.key.clone(),
            runtime: runtime_name(plugin.kind).into(),
        })
        .collect();
    modules.sort_by_key(|module| module.name.to_lowercase());
    let output_mode = serde_json::to_value(&parsed.osc.output_mode)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default();
    Ok(Config {
        path: config.display().to_string(),
        module: active_key(&parsed.module.active, &discovered),
        modules,
        // The API says, as it knows where the host is.
        dotnet_host: false,
        output_mode,
        output_modes: OUTPUT_MODES.map(String::from).to_vec(),
        send_address: parsed.osc.send_address,
        send_port: parsed.osc.send_port,
        max_fps: parsed.max_fps,
        smoothing_enabled: parsed.mutator.enabled,
        smoothing: parsed.mutator.smoothness,
        tuning: tuning(&parsed.mutator),
        adjustment_groups: ADJUSTMENT_GROUPS
            .iter()
            .map(|group| {
                let [min, max] = group.target.full_range();
                AdjustmentGroup {
                    key: group.key.into(),
                    label: group.label.into(),
                    min,
                    max,
                }
            })
            .collect(),
    })
}

/// The tracking tuning in `mutator`, as the API says it.
fn tuning(mutator: &MutatorConfig) -> Tuning {
    let filter = &mutator.filter;
    let correctors = &mutator.correctors;
    Tuning {
        filter: FilterTuning {
            min_cutoff: filter.min_cutoff,
            beta: filter.beta,
            d_cutoff: filter.d_cutoff,
            head: filter.head,
        },
        correctors: CorrectorsTuning {
            enabled: correctors.enabled,
            mouth_closed_clamp: correctors.mouth_closed_clamp,
            lip_suck_limiter: correctors.lip_suck_limiter,
            eyelid_blend: correctors.eyelid_blend,
            eye_look_symmetrize: correctors.eye_look_symmetrize,
        },
        adjustment: AdjustmentTuning {
            enabled: mutator.adjustment.enabled,
            ranges: mutator.adjustment.ranges.clone(),
        },
    }
}

/// Why `tuning` can't be saved, when it can't.
fn check_tuning(tuning: &Tuning) -> Result<(), String> {
    let filter = &tuning.filter;
    if let Some(cutoff) = filter.min_cutoff {
        if !cutoff.is_finite() || cutoff <= 0.0 {
            return Err("The minimum cutoff must be above 0.".into());
        }
    }
    if let Some(beta) = filter.beta {
        if !beta.is_finite() || beta < 0.0 {
            return Err("Beta can't be below 0.".into());
        }
    }
    if !filter.d_cutoff.is_finite() || filter.d_cutoff <= 0.0 {
        return Err("The derivative cutoff must be above 0.".into());
    }
    let blend = tuning.correctors.eyelid_blend;
    if !blend.is_finite() || !(0.0..=1.0).contains(&blend) {
        return Err("Eyelid blend must be between 0 and 1.".into());
    }
    for (key, &range) in &tuning.adjustment.ranges {
        let group =
            adjustment_group(key).ok_or_else(|| format!("{key} isn't an adjustment group."))?;
        let [min, max] = group.target.full_range();
        let [floor, ceil] = range;
        if !valid_range(range) || floor < min || ceil > max {
            return Err(format!(
                "The {} range must rise from {min} to at most {max}.",
                group.label
            ));
        }
    }
    Ok(())
}

/// Writes `tuning` into `mutator`, keeping anything else there.
fn write_tuning(mutator: &mut Map<String, Value>, tuning: &Tuning) {
    let filter = section(mutator, "filter");
    for (key, value) in [
        ("min_cutoff", tuning.filter.min_cutoff),
        ("beta", tuning.filter.beta),
    ] {
        match value {
            Some(value) => filter.insert(key.into(), json!(tidy(value))),
            None => filter.remove(key),
        };
    }
    filter.insert("d_cutoff".into(), json!(tidy(tuning.filter.d_cutoff)));
    filter.insert("head".into(), json!(tuning.filter.head));

    let fixes = &tuning.correctors;
    let correctors = section(mutator, "correctors");
    correctors.insert("enabled".into(), json!(fixes.enabled));
    correctors.insert("mouth_closed_clamp".into(), json!(fixes.mouth_closed_clamp));
    correctors.insert("lip_suck_limiter".into(), json!(fixes.lip_suck_limiter));
    correctors.insert("eyelid_blend".into(), json!(tidy(fixes.eyelid_blend)));
    correctors.insert(
        "eye_look_symmetrize".into(),
        json!(fixes.eye_look_symmetrize),
    );

    // A group at its full range changes nothing, so it isn't written.
    let ranges: Map<String, Value> = tuning
        .adjustment
        .ranges
        .iter()
        .filter(|(key, range)| {
            adjustment_group(key).is_some_and(|group| **range != group.target.full_range())
        })
        .map(|(key, [floor, ceil])| (key.clone(), json!([tidy(*floor), tidy(*ceil)])))
        .collect();
    let adjustment = section(mutator, "adjustment");
    adjustment.insert("enabled".into(), json!(tuning.adjustment.enabled));
    adjustment.insert("ranges".into(), Value::Object(ranges));
}

/// Checks `patch` and writes it. `modules` are the plugins found in
/// `plugins`; a module is saved as its key.
pub fn apply(
    config: &Path,
    patch: &ConfigPatch,
    modules: &[plugin_loader::DiscoveredPlugin],
) -> Result<(), String> {
    let module = match &patch.module {
        Some(module) => Some(
            plugin_loader::find_plugin(modules, module)
                .map(|plugin| plugin.key.clone())
                .ok_or_else(|| format!("There's no tracking module called {module} in plugins."))?,
        ),
        None => None,
    };
    if let Some(mode) = &patch.output_mode {
        if !OUTPUT_MODES.contains(&mode.as_str()) {
            return Err(format!("{mode} isn't an output VRFaceTracking supports."));
        }
    }
    if let Some(address) = &patch.send_address {
        if address.trim().is_empty() {
            return Err("Enter the address to send tracking to.".into());
        }
    }
    if patch.send_port == Some(0) {
        return Err("The port can't be 0.".into());
    }
    if let Some(fps) = patch.max_fps {
        if !fps.is_finite() || !(0.0..=1000.0).contains(&fps) {
            return Err("The frame rate limit must be between 0 (no limit) and 1000.".into());
        }
    }
    if let Some(smoothing) = patch.smoothing {
        if !smoothing.is_finite() || !(0.0..=1.0).contains(&smoothing) {
            return Err("Smoothing must be between 0 and 1.".into());
        }
    }
    if let Some(tuning) = &patch.tuning {
        check_tuning(tuning)?;
    }
    edit(config, |root| {
        if let Some(module) = &module {
            section(root, "module").insert("active".into(), json!(module));
        }
        if let Some(mode) = &patch.output_mode {
            section(root, "osc").insert("output_mode".into(), json!(mode));
        }
        if let Some(address) = &patch.send_address {
            section(root, "osc").insert("send_address".into(), json!(address.trim()));
        }
        if let Some(port) = patch.send_port {
            section(root, "osc").insert("send_port".into(), json!(port));
        }
        if let Some(fps) = patch.max_fps {
            let limit = if fps > 0.0 {
                json!(tidy(fps))
            } else {
                Value::Null
            };
            root.insert("max_fps".into(), limit);
        }
        if let Some(enabled) = patch.smoothing_enabled {
            section(root, "mutator").insert("enabled".into(), json!(enabled));
        }
        if let Some(smoothing) = patch.smoothing {
            section(root, "mutator").insert("smoothness".into(), json!(tidy(smoothing)));
        }
        if let Some(tuning) = &patch.tuning {
            write_tuning(section(root, "mutator"), tuning);
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vrft_config_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn enabling_keeps_the_rest_of_the_config() {
        let dir = temp_dir("enable");
        let config = dir.join("config.json");
        std::fs::write(
            &config,
            r#"{"module": {"active": "vd_module.dll"}, "extensions": {"quest-pro": {"note": 1}}}"#,
        )
        .unwrap();
        write_enabled(&config, "quest-pro", false).unwrap();
        let written: Value = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
        assert_eq!(written["module"]["active"], "vd_module.dll");
        assert_eq!(
            written["extensions"]["quest-pro"],
            json!({"note": 1, "enabled": false})
        );

        let missing = dir.join("missing.json");
        write_enabled(&missing, "quest-pro", true).unwrap();
        let written: Value = serde_json::from_slice(&std::fs::read(&missing).unwrap()).unwrap();
        assert_eq!(
            written,
            json!({"extensions": {"quest-pro": {"enabled": true}}})
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_patch_changes_only_its_settings_and_is_checked() {
        let dir = temp_dir("patch");
        let config = dir.join("config.json");
        std::fs::write(
            &config,
            r#"{"module": {"active": "vd_module.dll"}, "osc": {"output_mode": "VRChat",
                "send_address": "127.0.0.1", "send_port": 9000}, "future": true}"#,
        )
        .unwrap();
        let plugin = |key: &str| plugin_loader::DiscoveredPlugin {
            name: key.rsplit('/').next().unwrap().into(),
            key: key.into(),
            path: PathBuf::from(key),
            kind: PluginKind::Native,
            manifest: None,
        };
        let modules = vec![plugin("vd_module.dll"), plugin("registry/abc/other.dll")];
        let patch = ConfigPatch {
            // A bare file name, as older apps send, is saved as the key.
            module: Some("other.dll".into()),
            output_mode: Some("Generic".into()),
            send_port: Some(9100),
            max_fps: Some(0.0),
            smoothing: Some(0.5),
            ..ConfigPatch::default()
        };
        apply(&config, &patch, &modules).unwrap();
        let written: Value = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
        assert_eq!(written["module"]["active"], "registry/abc/other.dll");
        assert_eq!(written["osc"]["output_mode"], "Generic");
        assert_eq!(written["osc"]["send_address"], "127.0.0.1");
        assert_eq!(written["osc"]["send_port"], 9100);
        assert!(written["max_fps"].is_null(), "0 means no limit");
        assert_eq!(written["mutator"]["smoothness"], 0.5);
        assert_eq!(written["future"], true, "unknown settings are kept");

        let bad = |patch: ConfigPatch| apply(&config, &patch, &modules).unwrap_err();
        assert!(bad(ConfigPatch {
            module: Some("missing.dll".into()),
            ..ConfigPatch::default()
        })
        .contains("no tracking module"));
        assert!(bad(ConfigPatch {
            output_mode: Some("Discord".into()),
            ..ConfigPatch::default()
        })
        .contains("isn't an output"));
        assert!(bad(ConfigPatch {
            output_mode: Some("Resonite".into()),
            ..ConfigPatch::default()
        })
        .contains("isn't an output"));
        assert!(bad(ConfigPatch {
            send_port: Some(0),
            ..ConfigPatch::default()
        })
        .contains("port"));

        let plugins = dir.join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();
        let view = view(&config, &plugins).unwrap();
        assert_eq!(
            view.module, "registry/abc/other.dll",
            "unmatched, it's left as saved"
        );
        assert_eq!(view.output_mode, "Generic");
        assert!(!view.output_modes.contains(&"Resonite".to_string()));
        assert_eq!(view.max_fps, None);
        assert!(view.modules.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tuning_is_read_written_and_checked() {
        let dir = temp_dir("tuning");
        let config = dir.join("config.json");
        std::fs::write(
            &config,
            r#"{"mutator": {"smoothness": 0.2, "filter": {"min_cutoff": 2.0, "note": 1},
                "adjustment": {"ranges": {"jaw": [0.0, 0.5]}}}}"#,
        )
        .unwrap();
        let plugins = dir.join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();
        let seen = view(&config, &plugins).unwrap();
        assert_eq!(seen.tuning.filter.min_cutoff, Some(2.0));
        assert!(seen.tuning.correctors.enabled);
        assert_eq!(seen.tuning.adjustment.ranges["jaw"], [0.0, 0.5]);
        assert_eq!(seen.adjustment_groups.len(), ADJUSTMENT_GROUPS.len());
        let head = seen
            .adjustment_groups
            .iter()
            .find(|group| group.key == "head_yaw")
            .unwrap();
        assert_eq!((head.min, head.max), (-1.0, 1.0));

        let mut tuning = seen.tuning.clone();
        tuning.filter.min_cutoff = None;
        tuning.filter.head = false;
        tuning.correctors.eyelid_blend = 0.3;
        tuning.adjustment.enabled = true;
        tuning
            .adjustment
            .ranges
            .insert("eye_wide".into(), [0.1, 1.0]);
        tuning
            .adjustment
            .ranges
            .insert("tongue_out".into(), [0.0, 1.0]);
        let patch = ConfigPatch {
            tuning: Some(tuning),
            ..ConfigPatch::default()
        };
        apply(&config, &patch, &[]).unwrap();
        let written: Value = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
        let mutator = &written["mutator"];
        assert_eq!(mutator["smoothness"], 0.2);
        assert!(mutator["filter"].get("min_cutoff").is_none());
        assert_eq!(mutator["filter"]["note"], 1, "unknown settings are kept");
        assert_eq!(mutator["filter"]["head"], false);
        assert_eq!(mutator["correctors"]["eyelid_blend"], 0.3);
        assert_eq!(mutator["adjustment"]["enabled"], true);
        assert_eq!(
            mutator["adjustment"]["ranges"],
            json!({"jaw": [0.0, 0.5], "eye_wide": [0.1, 1.0]}),
            "a full range isn't written"
        );

        let bad = |change: fn(&mut Tuning)| {
            let mut tuning = Tuning::default();
            change(&mut tuning);
            let patch = ConfigPatch {
                tuning: Some(tuning),
                ..ConfigPatch::default()
            };
            apply(&config, &patch, &[]).unwrap_err()
        };
        assert!(bad(|t| t.filter.min_cutoff = Some(0.0)).contains("cutoff"));
        assert!(bad(|t| t.filter.beta = Some(-1.0)).contains("Beta"));
        assert!(bad(|t| t.correctors.eyelid_blend = 2.0).contains("blend"));
        assert!(bad(|t| {
            t.adjustment.ranges.insert("nope".into(), [0.0, 1.0]);
        })
        .contains("isn't an adjustment group"));
        assert!(bad(|t| {
            t.adjustment.ranges.insert("jaw".into(), [0.6, 0.4]);
        })
        .contains("range"));
        assert!(bad(|t| {
            t.adjustment.ranges.insert("jaw".into(), [-0.5, 1.0]);
        })
        .contains("range"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn modules_get_friendly_names() {
        assert_eq!(friendly_name("vd_module.dll"), "Virtual Desktop");
        assert_eq!(friendly_name("Meta_Quest_Pro.dll"), "Meta Quest Pro");
    }
}
