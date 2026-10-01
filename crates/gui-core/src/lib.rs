//! What the desktop app and its extensions share.
//!
//! - **The extension contract**, [`extension`]: the [`GuiExtension`] trait an
//!   extension implements, what the app hands it, and the types its methods
//!   use. An extension builds against this.
//! - **A shared toolkit** extensions may use to look and behave like the rest
//!   of the app: the daemon [`client`] and its polled state ([`live`]), the
//!   [`launcher`], readings ([`summary`]), [`widgets`] and their [`palette`],
//!   the [`logs`], and helpers for [`paths`] and [`processes`]. [`nav`] is
//!   the app's navigation, which extensions reach through
//!   [`extension::open_page`].
//!
//! [`GuiExtension`]: extension::GuiExtension
pub mod client;
pub mod extension;
pub mod launcher;
pub mod live;
pub mod logs;
pub mod nav;
pub mod palette;
pub mod paths;
pub mod processes;
pub mod summary;
pub mod widgets;

use gpui_kit::Window;

// Text in `locales/`, in the locale the app chose. Keys missing from a
// language fall back to English.
rust_i18n::i18n!("locales", fallback = "en");

pub const NAV_WIDTH: f32 = 240.;
pub const PAGE_PADDING: f32 = 36.;
pub const PAGE_MAX_WIDTH: f32 = 1120.;

/// Width a page's content gets: the window less the navigation and the page
/// padding, up to the page's maximum.
pub fn content_width(window: &Window) -> f32 {
    (window.viewport_size().width.as_f32() - NAV_WIDTH).min(PAGE_MAX_WIDTH) - 2. * PAGE_PADDING
}

/// Every key a crate's sources look up with `t!("...")`, for a test that
/// each has English text; rust-i18n shows a missing key's name instead.
/// `src` is the crate's source folder.
pub fn translation_keys(src: &std::path::Path) -> Vec<String> {
    let mut keys = Vec::new();
    let mut folders = vec![src.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for path in entries.flatten().map(|entry| entry.path()) {
            if path.is_dir() {
                folders.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                for at in text.match_indices("t!(").map(|(at, _)| at) {
                    // Not the end of another macro, such as `format!(`.
                    let before = text[..at].chars().next_back();
                    if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                        continue;
                    }
                    let rest = &text[at + 3..];
                    let Some(literal) = rest.trim_start().strip_prefix('"') else {
                        continue;
                    };
                    let Some(key) = literal.split('"').next() else {
                        continue;
                    };
                    // Only literals shaped like keys, such as `home.stop`.
                    let segments = key.split('.');
                    if key.contains('.')
                        && segments.clone().all(|segment| {
                            !segment.is_empty()
                                && segment
                                    .chars()
                                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
                        })
                    {
                        keys.push(key.to_owned());
                    }
                }
            }
        }
    }
    keys.sort();
    keys.dedup();
    keys
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_key_has_english_text() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let missing: Vec<_> = super::translation_keys(&src)
            .into_iter()
            .filter(|key| super::_rust_i18n_try_translate("en", key).is_none())
            .collect();
        assert!(missing.is_empty(), "no English text for {missing:?}");
    }
}
