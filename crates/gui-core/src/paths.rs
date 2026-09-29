//! Where this app is, and the development checkout it may have been built in.
use std::path::{Path, PathBuf};

/// The folder holding this app, where a release keeps `vrft_d.exe` and the
/// headset app. An installed copy's updates replace it, so what the app
/// writes goes in [`data_dir`].
pub fn app_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(Path::to_path_buf)
}

/// The folder the daemon keeps what it writes in, and where the app keeps
/// what it downloads: an installed copy's data folder, which updates leave
/// alone (see [`vrft_protocol::layout`]), or else the app's own folder.
pub fn data_dir() -> Option<PathBuf> {
    let app_dir = app_dir()?;
    Some(vrft_protocol::layout::installed_data_dir(&app_dir).unwrap_or(app_dir))
}

/// The `config.json` the daemon reads, from its working folder: the data
/// folder, or the repository root for a development build, which moves
/// itself there.
pub fn config_file() -> Option<PathBuf> {
    let app_dir = app_dir()?;
    let folder = checkout_root(&app_dir)
        .filter(|root| root.join("crates/daemon").is_dir())
        .map(Path::to_path_buf)
        .or_else(data_dir)?;
    Some(folder.join("config.json"))
}

/// The repository a development build lives in, from its
/// `<repo>/target/[<triple>/]<profile>/` folder. `None` for a release.
pub fn checkout_root(app_dir: &Path) -> Option<&Path> {
    app_dir
        .ancestors()
        .find(|dir| dir.file_name().is_some_and(|name| name == "target"))?
        .parent()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_development_build_finds_its_checkout() {
        let repo = Path::new("C:/work/VRFaceTracking");
        assert_eq!(checkout_root(&repo.join("target/debug")), Some(repo));
        assert_eq!(
            checkout_root(&repo.join("target/x86_64-pc-windows-msvc/release")),
            Some(repo)
        );
        assert_eq!(checkout_root(Path::new("C:/Games/VRFaceTracking")), None);
    }
}
