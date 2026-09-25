//! The headset app package this VRFT offers to install, and how it compares
//! with the version on the headset.
use crate::adb::InstalledApp;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A VRFT release carries the headset app it works with in this folder,
/// named `vrft-questpro-camera-<version>.apk`.
const BUNDLED_DIR: &str = "headset-app";
const BUNDLED_PREFIX: &str = "vrft-questpro-camera-";
/// Where Gradle builds the headset app in a development checkout.
const BUILD_OUTPUTS: &str = "android/questpro-camera/app/build/outputs/apk";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub path: PathBuf,
    /// `2026.9.0`, or `dev-727dfc4` for a development build, when known.
    pub version: Option<String>,
    pub origin: PackageOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageOrigin {
    /// Came with this VRFT release.
    Bundled,
    /// Built in the checkout this development build of the app runs from.
    LocalBuild,
    /// Picked by the user.
    Chosen,
}

impl Package {
    pub fn chosen(path: PathBuf) -> Self {
        Self {
            path,
            version: None,
            origin: PackageOrigin::Chosen,
        }
    }

    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// The version, or the file name when the version isn't known.
    pub fn describe(&self) -> String {
        self.version.clone().unwrap_or_else(|| self.file_name())
    }
}

/// The headset app that came with this VRFT or, for a development build of
/// this app, the newest one built in its checkout.
pub fn find(app_dir: &Path) -> Option<Package> {
    bundled(&app_dir.join(BUNDLED_DIR)).or_else(|| {
        crate::paths::checkout_root(app_dir).and_then(|root| local_build(&root.join(BUILD_OUTPUTS)))
    })
}

fn bundled(dir: &Path) -> Option<Package> {
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let version = bundled_version(path.file_name()?.to_str()?)?.to_string();
            Some(Package {
                path,
                version: Some(version),
                origin: PackageOrigin::Bundled,
            })
        })
        .max_by_key(|package| package.version.as_deref().and_then(version_code))
}

fn bundled_version(file_name: &str) -> Option<&str> {
    file_name
        .strip_prefix(BUNDLED_PREFIX)?
        .strip_suffix(".apk")
        .filter(|version| !version.is_empty())
}

/// The most recently built installable APK among Gradle's variants, with the
/// version Gradle records beside it.
fn local_build(outputs: &Path) -> Option<Package> {
    ["debug", "release"]
        .into_iter()
        .filter_map(|variant| {
            let dir = outputs.join(variant);
            let metadata = fs::read_to_string(dir.join("output-metadata.json")).ok()?;
            let (file, version) = parse_output_metadata(&metadata)?;
            let path = dir.join(file);
            let built = fs::metadata(&path).ok()?.modified().ok()?;
            Some((
                built,
                Package {
                    path,
                    version: Some(version),
                    origin: PackageOrigin::LocalBuild,
                },
            ))
        })
        .max_by_key(|(built, _): &(SystemTime, Package)| *built)
        .map(|(_, package)| package)
}

/// The APK and its version from Gradle's `output-metadata.json`. An unsigned
/// release build can't be installed, so it doesn't count.
fn parse_output_metadata(json: &str) -> Option<(String, String)> {
    #[derive(Deserialize)]
    struct Metadata {
        elements: Vec<Element>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Element {
        version_name: String,
        output_file: String,
    }
    let element = serde_json::from_str::<Metadata>(json)
        .ok()?
        .elements
        .into_iter()
        .next()?;
    (!element.output_file.contains("unsigned"))
        .then_some((element.output_file, element.version_name))
}

/// A release's `versionCode`, worked out as `build.gradle` does:
/// `2026.9.0` is 20260900.
pub fn version_code(version: &str) -> Option<u32> {
    let mut parts = version.splitn(3, '.');
    let year: u32 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let count: u32 = parts.next()?.parse().ok()?;
    ((1000..=9999).contains(&year) && (1..=12).contains(&month) && count <= 99)
        .then_some(year * 10_000 + month * 100 + count)
}

/// How the package compares with the app on the headset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comparison {
    NotInstalled,
    /// The package is newer.
    Update,
    Same,
    /// The headset has a newer version.
    Older,
    /// A development build is involved, so the versions don't say which is
    /// newer.
    Unknown,
}

pub fn compare(installed: Option<&InstalledApp>, package: &Package) -> Comparison {
    let Some(installed) = installed else {
        return Comparison::NotInstalled;
    };
    let Some(version) = package.version.as_deref() else {
        return Comparison::Unknown;
    };
    if version == installed.version_name {
        return Comparison::Same;
    }
    // Only releases carry comparable codes; development builds share theirs.
    match (version_code(version), version_code(&installed.version_name)) {
        (Some(offered), Some(_)) => match offered.cmp(&installed.version_code) {
            std::cmp::Ordering::Greater => Comparison::Update,
            std::cmp::Ordering::Equal => Comparison::Same,
            std::cmp::Ordering::Less => Comparison::Older,
        },
        _ => Comparison::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(version: &str) -> InstalledApp {
        InstalledApp {
            version_name: version.into(),
            version_code: version_code(version).unwrap_or(2),
        }
    }

    fn package(version: Option<&str>) -> Package {
        Package {
            path: PathBuf::from("headset-app/app.apk"),
            version: version.map(str::to_string),
            origin: PackageOrigin::Bundled,
        }
    }

    #[test]
    fn version_codes_match_build_gradle() {
        assert_eq!(version_code("2026.9.0"), Some(20260900));
        assert_eq!(version_code("2026.12.17"), Some(20261217));
        assert_eq!(version_code("2026.13.0"), None);
        assert_eq!(version_code("2026.9.100"), None);
        assert_eq!(version_code("0.2"), None);
        assert_eq!(version_code("dev-727dfc4"), None);
        // A build after a release carries more than a version.
        assert_eq!(version_code("2026.9.0-3-gabc1234"), None);
    }

    #[test]
    fn bundled_file_names_carry_the_version() {
        assert_eq!(
            bundled_version("vrft-questpro-camera-2026.9.0.apk"),
            Some("2026.9.0")
        );
        assert_eq!(bundled_version("vrft-questpro-camera-.apk"), None);
        assert_eq!(bundled_version("app-debug.apk"), None);
    }

    #[test]
    fn compares_releases_and_leaves_development_builds_unknown() {
        let offer = package(Some("2026.10.0"));
        assert_eq!(compare(None, &offer), Comparison::NotInstalled);
        assert_eq!(
            compare(Some(&installed("2026.9.0")), &offer),
            Comparison::Update
        );
        assert_eq!(
            compare(Some(&installed("2026.10.0")), &offer),
            Comparison::Same
        );
        assert_eq!(
            compare(Some(&installed("2026.11.0")), &offer),
            Comparison::Older
        );
        assert_eq!(
            compare(Some(&installed("0.2")), &offer),
            Comparison::Unknown
        );
        assert_eq!(
            compare(Some(&installed("2026.9.0")), &package(None)),
            Comparison::Unknown
        );
    }

    #[test]
    fn reads_gradle_output_metadata() {
        let json = r#"{"version": 3, "applicationId": "io.github.matty.vrft.questprocamera",
            "elements": [{"type": "SINGLE", "versionCode": 2, "versionName": "dev-727dfc4",
                          "outputFile": "app-debug.apk"}]}"#;
        assert_eq!(
            parse_output_metadata(json),
            Some(("app-debug.apk".into(), "dev-727dfc4".into()))
        );
        let unsigned =
            r#"{"elements": [{"versionName": "dev", "outputFile": "app-release-unsigned.apk"}]}"#;
        assert_eq!(parse_output_metadata(unsigned), None);
    }
}
