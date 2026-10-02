//! Running an installed copy, whose executables are in `<root>/current/`,
//! which every update replaces, from `<root>/data/`, which updates leave
//! alone. See [`vrft_protocol::layout`].
use log::{info, warn};
use std::fs;
use std::io;
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
        copy(from, to);
        return;
    }
    let Ok(entries) = fs::read_dir(from) else {
        return;
    };
    for entry in entries.flatten() {
        copy_changed(&entry.path(), &to.join(entry.file_name()));
    }
}

/// Copies `from` to `to` when they differ, warning when it can't.
fn copy(from: &Path, to: &Path) {
    if let Err(error) = copy_if_changed(from, to) {
        warn!(
            "Couldn't copy {} to {}, so the one there may be out of date: {error}",
            from.display(),
            to.display()
        );
    }
}

/// Copies file `from` to `to`, making its folder, unless `to` is already a
/// copy of it: `fs::copy` keeps the modification time, so a copy matches its
/// original until the original changes. Returns whether it copied.
pub fn copy_if_changed(from: &Path, to: &Path) -> io::Result<bool> {
    let source = fs::metadata(from)?;
    let same = fs::metadata(to).is_ok_and(|copy| {
        copy.len() == source.len() && copy.modified().ok() == source.modified().ok()
    });
    if same {
        return Ok(false);
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(from, to)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::temp_dir;

    #[test]
    fn shipped_files_are_refreshed_and_settings_kept() {
        let root = temp_dir("installed");
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
        // Written within the same clock tick, the two can have the same
        // modification time, so the size tells them apart.
        fs::write(data.join("plugins/vd_module.dll"), b"older").unwrap();
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
