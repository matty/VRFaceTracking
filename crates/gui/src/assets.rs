//! The component library's default icons plus the few extra Lucide icons this
//! app uses. Only the listed SVGs are embedded.
use gpui_kit::assets::{icon_assets, Assets};
use gpui_kit::{AssetSource, Result, SharedString};
use std::borrow::Cow;

icon_assets!(
    ExtraIcons,
    [
        Activity,
        Cable,
        Camera,
        CameraOff,
        CircleStop,
        Crosshair,
        Download,
        FaceGrinning,
        Glasses,
        House,
        Lightbulb,
        ListChecks,
        MessageCircle,
        Monitor,
        Move,
        Package,
        Pencil,
        Power,
        Puzzle,
        Radio,
        RefreshCw,
        RotateCcw,
        Route,
        ScanFace,
        ScanEye,
        ScrollText,
        Send,
        SkipForward,
        SlidersHorizontal,
        Sparkles,
        Timer,
        Trash,
        Unplug,
        Usb,
        Volume2,
        Wifi,
    ]
);

/// VRFaceTracking's own mark, the face from its app icon, for the
/// navigation. The icon's sources are beside it in `resources/`.
pub const MARK: &str = "brand/mark.svg";

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path == MARK {
            return Ok(Some(Cow::Borrowed(include_bytes!("../resources/mark.svg"))));
        }
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        if MARK.starts_with(path) {
            paths.push(MARK.into());
        }
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// `ListChecks` as its file, `list-checks`; `Volume2` as `volume-2`.
    fn file_name(icon: &str) -> String {
        let mut out = String::new();
        let mut previous: Option<char> = None;
        for c in icon.chars() {
            let boundary = match previous {
                Some(p) => c.is_ascii_uppercase() || (c.is_ascii_digit() && !p.is_ascii_digit()),
                None => false,
            };
            if boundary {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
            previous = Some(c);
        }
        out
    }

    /// Every icon name used in the Rust files under `dir`.
    fn icons_used(dir: &Path, found: &mut Vec<(String, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                icons_used(&path, found);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                for (index, _) in text.match_indices("IconName::") {
                    let name: String = text[index + "IconName::".len()..]
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric())
                        .collect();
                    if name.starts_with(|c: char| c.is_ascii_uppercase()) {
                        found.push((name, path.display().to_string()));
                    }
                }
            }
        }
    }

    #[test]
    fn icon_file_names() {
        assert_eq!(file_name("ListChecks"), "list-checks");
        assert_eq!(file_name("Volume2"), "volume-2");
        assert_eq!(file_name("Trash"), "trash");
    }

    /// An icon the app uses but doesn't embed shows as nothing, with only
    /// an error in the log to say so.
    #[test]
    fn every_icon_the_app_uses_is_embedded() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut used = Vec::new();
        for dir in ["crates/gui/src", "crates/gui-core/src", "extensions"] {
            icons_used(&root.join(dir), &mut used);
        }
        assert!(!used.is_empty());
        let missing: Vec<String> = used
            .iter()
            .filter(|(name, _)| {
                let path = format!("icons/{}.svg", file_name(name));
                AppAssets.load(&path).ok().flatten().is_none()
            })
            .map(|(name, file)| format!("{name} in {file}"))
            .collect();
        assert!(
            missing.is_empty(),
            "add these to icon_assets! in assets.rs: {missing:#?}"
        );
    }
}
