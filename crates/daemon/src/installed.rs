//! Running an installed copy, whose executables are in `<root>/current/`,
//! which every update replaces, from `<root>/data/`, which updates leave
//! alone. See [`vrft_protocol::layout`].
use log::{info, warn};
use std::fs;
use std::path::{Path, PathBuf};

/// What a package ships that the daemon reads from its working folder. Each
/// start copies in whatever changed, so an update's newer modules and models
/// replace the old ones, while modules and models added since stay.
const SHIPPED: [&str; 3] = ["plugins", "runtime", "models"];
/// Copied only when missing, so the settings people chose survive updates.
const SEEDED: [&str; 1] = ["config.json"];

/// Makes an installed copy's data folder the working directory, filling it
/// with what the package ships first. Returns the data folder, or `None`
/// when this isn't an installed copy.
pub fn use_data_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let app_dir = exe.parent()?;
    let data = vrft_protocol::layout::installed_data_dir(app_dir)?;
    if let Err(error) = fs::create_dir_all(&data) {
        warn!("Couldn't create {}: {error}", data.display());
        return None;
    }
    fill(app_dir, &data);
    match std::env::set_current_dir(&data) {
        Ok(()) => {
            info!(
                "Installed copy; using {} as the working directory",
                data.display()
            );
            Some(data)
        }
        Err(error) => {
            warn!("Couldn't move to {}: {error}", data.display());
            None
        }
    }
}

/// Copies what `app_dir` ships into `data`.
fn fill(app_dir: &Path, data: &Path) {
    for name in SHIPPED {
        copy_changed(&app_dir.join(name), &data.join(name));
    }
    for name in SEEDED {
        let (from, to) = (app_dir.join(name), data.join(name));
        if from.is_file() && !to.exists() {
            copy(&from, &to);
        }
    }
}

/// Copies each file under `from` whose copy under `to` is missing or differs.
fn copy_changed(from: &Path, to: &Path) {
    if from.is_file() {
        let same = match (fs::metadata(from), fs::metadata(to)) {
            (Ok(a), Ok(b)) => a.len() == b.len() && a.modified().ok() == b.modified().ok(),
            _ => false,
        };
        if !same {
            copy(from, to);
        }
        return;
    }
    let Ok(entries) = fs::read_dir(from) else {
        return;
    };
    for entry in entries.flatten() {
        copy_changed(&entry.path(), &to.join(entry.file_name()));
    }
}

fn copy(from: &Path, to: &Path) {
    let result = to
        .parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| fs::copy(from, to));
    if let Err(error) = result {
        warn!(
            "Couldn't copy {} to {}, so the one there may be out of date: {error}",
            from.display(),
            to.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_files_are_refreshed_and_settings_kept() {
        let root = std::env::temp_dir().join(format!("vrft_installed_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let app = root.join("current");
        let data = root.join("data");
        fs::create_dir_all(app.join("plugins/sub")).unwrap();
        fs::create_dir_all(app.join("models/quest-pro")).unwrap();
        fs::create_dir_all(data.join("plugins")).unwrap();
        fs::write(app.join("plugins/vd_module.dll"), b"new").unwrap();
        fs::write(app.join("plugins/sub/other.dll"), b"sub").unwrap();
        fs::write(app.join("models/quest-pro/eye.json"), b"model").unwrap();
        fs::write(app.join("config.json"), b"{}").unwrap();
        fs::write(app.join("vrft_d.exe"), b"exe").unwrap();
        fs::write(data.join("plugins/vd_module.dll"), b"old").unwrap();
        fs::write(data.join("plugins/mine.dll"), b"mine").unwrap();
        fs::write(data.join("config.json"), b"{\"max_fps\": 30}").unwrap();

        fill(&app, &data);
        let read = |path: &str| fs::read(data.join(path)).unwrap();
        assert_eq!(read("plugins/vd_module.dll"), b"new");
        assert_eq!(read("plugins/sub/other.dll"), b"sub");
        assert_eq!(read("models/quest-pro/eye.json"), b"model");
        // Added by the user, and chosen by the user.
        assert_eq!(read("plugins/mine.dll"), b"mine");
        assert_eq!(read("config.json"), b"{\"max_fps\": 30}");
        // Only what the daemon reads from its working folder.
        assert!(!data.join("vrft_d.exe").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
