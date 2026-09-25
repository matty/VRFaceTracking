//! Where this app is, and the development checkout it may have been built in.
use std::path::{Path, PathBuf};

/// The folder holding this app, where a release keeps `vrft_d.exe` and the
/// headset app.
pub fn app_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(Path::to_path_buf)
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
        assert_eq!(checkout_root(Path::new("C:/Games/VRFT")), None);
    }
}
