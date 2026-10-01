//! Running a development build, from `<repo>/target/[<triple>/]<profile>/`,
//! against the repository's `config.json` and `plugins/`.
use log::{info, warn};
use std::fs;
use std::path::{Path, PathBuf};

/// A development build lives in `<repo>/target/[<triple>/]<profile>/`. Started
/// from there, for example by double-clicking it, the working directory has
/// no config, plugins, Quest Pro models or recordings, so use the repository
/// root instead. Release packages keep the executable beside those files and
/// are unaffected, as is any run started from outside the target directory.
///
/// Returns the folder cargo built this executable, and the modules beside it,
/// into when the working directory is the repository root, whether it
/// already was or has just been moved there.
pub fn use_repository_root() -> Option<PathBuf> {
    let (Ok(exe), Ok(cwd)) = (std::env::current_exe(), std::env::current_dir()) else {
        return None;
    };
    let build = exe.parent()?;
    let target = build
        .ancestors()
        .find(|dir| dir.file_name().is_some_and(|name| name == "target"))?;
    let root = target.parent()?;
    if !root.join("Cargo.toml").is_file() || !root.join("crates/daemon").is_dir() {
        return None;
    }
    let (Ok(cwd), Ok(target), Ok(canonical_root)) = (
        cwd.canonicalize(),
        target.canonicalize(),
        root.canonicalize(),
    ) else {
        return None;
    };
    if cwd == canonical_root {
        return Some(build.to_path_buf());
    }
    if cwd.starts_with(&target) && std::env::set_current_dir(root).is_ok() {
        info!(
            "Development build started inside {}; using the repository root {} as the working directory",
            target.display(),
            root.display()
        );
        return Some(build.to_path_buf());
    }
    None
}

/// Copies the workspace's tracking modules, the crates under `modules`, from
/// `build`, where cargo leaves them beside the executable, into `plugins`,
/// where the daemon loads them from, as a release package has them. Only
/// those that changed are copied, so a module rebuilt since the daemon last
/// started is picked up by the next start.
pub fn install_modules(build: &Path, modules: &Path, plugins: &Path) {
    for name in module_names(modules) {
        let file = format!(
            "{}{name}{}",
            std::env::consts::DLL_PREFIX,
            std::env::consts::DLL_SUFFIX
        );
        let built = build.join(&file);
        let installed = plugins.join(&file);
        // Not built in this profile, or not yet.
        let Ok(built_meta) = fs::metadata(&built) else {
            continue;
        };
        if fs::metadata(&installed).is_ok_and(|meta| same_file(&built_meta, &meta)) {
            continue;
        }
        match fs::copy(&built, &installed) {
            Ok(_) => info!("Copied {file} from this build into {}", plugins.display()),
            Err(error) => warn!(
                "Couldn't copy {} into {}, so the one there may be out of date: {error}",
                built.display(),
                plugins.display()
            ),
        }
    }
}

/// The host for managed (.NET / VRCFT) modules the repository keeps, at
/// `dotnet/publish/VrcftRuntime.exe` under `root`, for a development build,
/// which has no `runtime/` folder as a release package does.
pub fn dotnet_host(root: &Path) -> Option<PathBuf> {
    let host = root.join("dotnet/publish/VrcftRuntime.exe");
    host.is_file().then_some(host)
}

/// `fs::copy` keeps the modification time, so a copy matches its original
/// until the original is rebuilt.
fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    a.len() == b.len() && a.modified().ok() == b.modified().ok()
}

/// The library name of each crate under `modules`.
fn module_names(modules: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(modules) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| fs::read_to_string(entry.path().join("Cargo.toml")).ok())
        .filter_map(|manifest| library_name(&manifest))
        .collect();
    names.sort();
    names
}

/// The `[lib]` `name` in a crate's `Cargo.toml`, or else the name cargo
/// gives the library, its package name with `_` for `-`.
fn library_name(manifest: &str) -> Option<String> {
    let mut section = "";
    let mut package = None;
    let mut library = None;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "name" {
            continue;
        }
        let value = value.trim().trim_matches('"').to_owned();
        match section {
            "[package]" => package = Some(value),
            "[lib]" => library = Some(value),
            _ => {}
        }
    }
    library.or_else(|| package.map(|name| name.replace('-', "_")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vrft_dev_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn library(name: &str) -> String {
        format!(
            "{}{name}{}",
            std::env::consts::DLL_PREFIX,
            std::env::consts::DLL_SUFFIX
        )
    }

    #[test]
    fn modules_are_named_by_their_library() {
        let manifest = "[package]\nname = \"vrft-vd-module\"\n\n[lib]\nname = \"vd_module\"\ncrate-type = [\"cdylib\"]\n\n[dependencies]\nname = \"not-this\"\n";
        assert_eq!(library_name(manifest).as_deref(), Some("vd_module"));
        let manifest = "[package]\nname = \"my-module\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n";
        assert_eq!(library_name(manifest).as_deref(), Some("my_module"));
    }

    #[test]
    fn the_workspace_modules_are_found() {
        let modules = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../modules");
        let names = module_names(&modules);
        assert!(names.contains(&"vd_module".to_string()), "{names:?}");
        assert!(names.contains(&"test_logger".to_string()), "{names:?}");
    }

    #[test]
    fn built_modules_are_copied_into_plugins_when_they_change() {
        let dir = temp_dir("install");
        let build = dir.join("target/debug");
        let modules = dir.join("modules");
        let plugins = dir.join("plugins");
        for path in [
            &build,
            &modules.join("mine"),
            &modules.join("unbuilt"),
            &plugins,
        ] {
            fs::create_dir_all(path).unwrap();
        }
        fs::write(
            modules.join("mine/Cargo.toml"),
            "[package]\nname = \"vrft-mine\"\n\n[lib]\nname = \"mine\"\n",
        )
        .unwrap();
        fs::write(
            modules.join("unbuilt/Cargo.toml"),
            "[package]\nname = \"unbuilt\"\n",
        )
        .unwrap();
        // Not a module, so left where it is.
        fs::write(build.join(library("other")), b"other").unwrap();
        fs::write(build.join(library("mine")), b"first").unwrap();

        install_modules(&build, &modules, &plugins);
        assert_eq!(fs::read(plugins.join(library("mine"))).unwrap(), b"first");
        assert!(!plugins.join(library("other")).exists());
        assert!(!plugins.join(library("unbuilt")).exists());

        fs::write(build.join(library("mine")), b"rebuilt").unwrap();
        install_modules(&build, &modules, &plugins);
        assert_eq!(fs::read(plugins.join(library("mine"))).unwrap(), b"rebuilt");
        fs::remove_dir_all(dir).unwrap();
    }
}
