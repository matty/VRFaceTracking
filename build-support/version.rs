//! The version a build stamps in as `VRFT_VERSION`, shared by the build
//! scripts of `vrft_d` and the desktop app. Versions are CalVer, `YYYY.M.N`.
//!
//! A release sets `VRFT_VERSION` in the environment. Any other build is a
//! development build of the next release, `YYYY.M.N-dev.C`: the version the
//! next release made today would get, and `C` commits since the last `v`
//! release tag. `.github/scripts/next-calver.sh` works out the same for CI.
use std::path::Path;
use std::process::Command;

pub fn stamp() {
    println!("cargo:rerun-if-env-changed=VRFT_VERSION");
    let version = std::env::var("VRFT_VERSION")
        .ok()
        .map(|version| version.trim().to_owned())
        .filter(|version| !version.is_empty())
        .or_else(dev_version)
        .unwrap_or_else(|| "0.0.0-dev".into());
    println!("cargo:rustc-env=VRFT_VERSION={version}");
}

fn dev_version() -> Option<String> {
    watch_git();
    // `v[0-9]*` leaves out the rolling `dev` tag and the headset app's tags.
    let commits = match git(&["describe", "--tags", "--match", "v[0-9]*", "--abbrev=0"]) {
        Some(last) => git(&["rev-list", "--count", &format!("{last}..HEAD")])?,
        None => git(&["rev-list", "--count", "HEAD"])?,
    };
    let (year, month) = utc_year_month();
    let month = format!("{year}.{month}");
    let tags = git(&["tag", "--list", &format!("v{month}.*")]).unwrap_or_default();
    let next = tags
        .lines()
        .filter_map(|tag| tag.strip_prefix(&format!("v{month}."))?.parse::<u32>().ok())
        .max()
        .map_or(0, |last| last + 1);
    Some(format!("{month}.{next}-dev.{commits}"))
}

/// Today's year and month in UTC, as the release workflow dates versions.
fn utc_year_month() -> (i64, u32) {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64);
    // Howard Hinnant's days-to-civil algorithm.
    let days = seconds.div_euclid(86_400) + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month as u32)
}

/// Reruns the build script when a commit or tag would change the version.
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
