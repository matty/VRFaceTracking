//! Where an installed copy of VRFT keeps what it writes.
//!
//! The installer (Velopack) puts the app in `<root>/current/`, beside its own
//! `Update.exe`, and replaces all of `current/` on every update. So an
//! installed copy runs the daemon in `<root>/data/` instead: `config.json`,
//! `plugins/`, models, recordings and logs live there and survive updates.
//! The daemon copies what the package ships into it as it starts. A copy
//! unpacked from a zip keeps everything beside the executables, as before.
use std::path::{Path, PathBuf};

/// The folder the installer replaces on every update.
const CURRENT_DIR: &str = "current";
/// The installer's own updater, beside `current/`.
const UPDATER: &str = "Update.exe";
/// The folder beside `current/` that updates leave alone.
pub const DATA_DIR: &str = "data";

/// The install's root, when `app_dir`, the folder holding the executables,
/// is an installed copy's `<root>/current/`.
pub fn installed_root(app_dir: &Path) -> Option<&Path> {
    if app_dir.file_name()? != CURRENT_DIR {
        return None;
    }
    let root = app_dir.parent()?;
    root.join(UPDATER).is_file().then_some(root)
}

/// The data folder of an installed copy whose executables are in `app_dir`.
pub fn installed_data_dir(app_dir: &Path) -> Option<PathBuf> {
    installed_root(app_dir).map(|root| root.join(DATA_DIR))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn an_installed_copy_keeps_its_data_beside_current() {
        let root = std::env::temp_dir().join(format!("vrft_layout_{}", std::process::id()));
        let current = root.join("current");
        fs::create_dir_all(&current).unwrap();
        // Without the installer's updater it's just a folder called current.
        assert_eq!(installed_data_dir(&current), None);
        fs::write(root.join("Update.exe"), b"").unwrap();
        assert_eq!(installed_data_dir(&current), Some(root.join("data")));
        assert_eq!(installed_data_dir(&root), None);
        fs::remove_dir_all(root).unwrap();
    }
}
