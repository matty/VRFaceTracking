//! VRFT desktop app. It shows what the daemon, `vrft_d`, is doing through the
//! daemon's local API, and does not track anything itself.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod assets;
mod config_file;
mod debug;
mod extension_switches;
mod extensions;
mod home;
mod logs;
mod modules;
mod settings;
mod setup;
mod shell;
mod theme;
mod tracking;
mod updates;

use gpui_kit::component::{Root, TitleBar};
use gpui_kit::{px, size, AppContext as _, TitlebarOptions, WindowBounds, WindowOptions};

// The app's text, in `locales/`. Keys missing from a language fall back to
// English.
rust_i18n::i18n!("locales", fallback = "en");

/// Shows the app in the system's language when it has that language, or
/// `VRFT_LANG` (such as `de`) when set; otherwise English. The locale is
/// global, so gui-core, the extensions and gpui-component's own widgets
/// follow it too.
fn choose_language() {
    let wanted = std::env::var("VRFT_LANG")
        .ok()
        .or_else(sys_locale::get_locale)
        .unwrap_or_default();
    if let Some(locale) = supported_locale(&wanted, &rust_i18n::available_locales!()) {
        rust_i18n::set_locale(&locale);
    }
}

/// The locale in `available` that best matches `wanted`, a BCP 47 tag such
/// as `de-AT`: the tag itself, else its language alone (`de`).
fn supported_locale(wanted: &str, available: &[impl AsRef<str>]) -> Option<String> {
    let wanted = wanted.replace('_', "-");
    let language = wanted.split('-').next().unwrap_or_default();
    let found = [wanted.as_str(), language].into_iter().find_map(|tag| {
        available
            .iter()
            .map(AsRef::as_ref)
            .find(|locale| !tag.is_empty() && locale.eq_ignore_ascii_case(tag))
            .map(str::to_owned)
    });
    found
}

/// For capturing pages in development: `VRFT_GUI_BOUNDS=x,y,width,height`
/// places the window there. Debug builds only.
fn dev_bounds() -> Option<WindowBounds> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let text = std::env::var("VRFT_GUI_BOUNDS").ok()?;
    let values: Vec<f32> = text
        .split(',')
        .map(|value| value.trim().parse().ok())
        .collect::<Option<_>>()?;
    let [x, y, width, height] = values[..] else {
        return None;
    };
    Some(WindowBounds::Windowed(gpui_kit::Bounds::new(
        gpui_kit::point(px(x), px(y)),
        size(px(width), px(height)),
    )))
}

fn main() {
    // First, as the installer runs the app with arguments to set it up,
    // update it or remove it, and this handles them and exits. It also
    // installs an update downloaded last time before the app opens.
    velopack::VelopackApp::build().run();
    logs::init_logging();
    choose_language();

    gpui_kit::application()
        .with_assets(assets::AppAssets)
        .run(|cx| {
            gpui_kit::init(cx);
            // On Windows GPUI keeps running without a window, so closing the
            // last one would leave the app, and the daemon it started, behind.
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let options = WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some(updates::app_name()),
                    ..TitleBar::title_bar_options()
                }),
                window_bounds: Some(
                    dev_bounds()
                        .unwrap_or_else(|| WindowBounds::centered(size(px(1240.), px(820.)), cx)),
                ),
                window_min_size: Some(size(px(720.), px(520.))),
                ..TitleBar::window_options()
            };
            cx.open_window(options, |window, cx| {
                theme::install(window, cx);
                let workspace = cx.new(|cx| shell::Workspace::new(window, cx));
                cx.new(|cx| Root::new(workspace, window, cx))
            })
            .expect("failed to open the VRFaceTracking window");
        });
}

#[cfg(test)]
mod tests {
    use super::supported_locale;

    #[test]
    fn every_key_has_english_text() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let missing: Vec<_> = vrft_gui_core::translation_keys(&src)
            .into_iter()
            .filter(|key| super::_rust_i18n_try_translate("en", key).is_none())
            .collect();
        assert!(missing.is_empty(), "no English text for {missing:?}");
    }

    #[test]
    fn picks_the_closest_available_locale() {
        let available = ["en", "de", "zh-CN"];
        assert_eq!(supported_locale("de-AT", &available).as_deref(), Some("de"));
        assert_eq!(
            supported_locale("zh_CN", &available).as_deref(),
            Some("zh-CN")
        );
        assert_eq!(supported_locale("EN", &available).as_deref(), Some("en"));
        assert_eq!(supported_locale("fr-FR", &available), None);
        assert_eq!(supported_locale("", &available), None);
    }
}
