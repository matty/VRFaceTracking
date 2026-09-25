//! The component library's default icons plus the few extra Lucide icons this
//! app uses. Only the listed SVGs are embedded.
use gpui_kit::assets::{icon_assets, Assets};
use gpui_kit::{AssetSource, Result, SharedString};
use std::borrow::Cow;

icon_assets!(
    ExtraIcons,
    [
        Activity,
        Camera,
        CameraOff,
        CircleStop,
        Download,
        Glasses,
        Package,
        Puzzle,
        ScanEye,
        ScanFace,
        ScrollText,
        Send,
        SkipForward,
        Trash,
        Unplug,
        Usb,
        Wifi,
    ]
);

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}
