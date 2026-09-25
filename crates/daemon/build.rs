//! Stamps `VRFT_VERSION` into the build. A release sets it in the environment;
//! otherwise it is `git describe` against the last `v` release tag, such as
//! `2026.9.0-14-g3f2a1c9`, or `dev-3f2a1c9` before the first release.
use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=VRFT_VERSION");
    let version = std::env::var("VRFT_VERSION")
        .ok()
        .filter(|version| !version.trim().is_empty())
        .or_else(describe)
        .unwrap_or_else(|| "dev".into());
    println!("cargo:rustc-env=VRFT_VERSION={}", version.trim());
}

fn describe() -> Option<String> {
    watch_git();
    // `v[0-9]*` leaves out the rolling `dev` tag and the headset app's `apk-v` tags.
    let described = git(&["describe", "--tags", "--match", "v[0-9]*", "--always"])?;
    Some(match described.strip_prefix('v') {
        Some(version) => version.to_owned(),
        None => format!("dev-{described}"),
    })
}

/// Reruns this script when a commit or tag would change the description.
fn watch_git() {
    let mut paths = vec!["HEAD".to_owned(), "packed-refs".into(), "refs/tags".into()];
    paths.extend(git(&["symbolic-ref", "-q", "HEAD"]));
    for path in paths {
        // Cargo reruns every build for a watched path that doesn't exist.
        if let Some(file) = git(&["rev-parse", "--git-path", &path]) {
            if Path::new(&file).exists() {
                println!("cargo:rerun-if-changed={file}");
            }
        }
    }
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (output.status.success() && !text.is_empty()).then(|| text.to_owned())
}
