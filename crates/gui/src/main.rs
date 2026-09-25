//! VRFT desktop app. It shows what the daemon, `vrft_d`, is doing through the
//! daemon's local API, and does not track anything itself.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod adb;
mod assets;
mod camera;
mod daemon;
mod eyes;
mod headset;
mod headset_app;
mod home;
mod launcher;
mod live;
mod paths;
mod processes;
mod shell;
mod summary;
mod widgets;

use gpui_kit::component::{Root, Theme, TitleBar};
use gpui_kit::{px, size, AppContext as _, TitlebarOptions, WindowBounds, WindowOptions};

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    gpui_kit::application()
        .with_assets(assets::AppAssets)
        .run(|cx| {
            gpui_kit::init(cx);
            let options = WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("VRFT".into()),
                    ..TitleBar::title_bar_options()
                }),
                window_bounds: Some(WindowBounds::centered(size(px(1180.), px(780.)), cx)),
                window_min_size: Some(size(px(880.), px(600.))),
                ..TitleBar::window_options()
            };
            cx.open_window(options, |window, cx| {
                Theme::sync_system_appearance(Some(window), cx);
                let workspace = cx.new(|cx| shell::Workspace::new(window, cx));
                cx.new(|cx| Root::new(workspace, window, cx))
            })
            .expect("failed to open the VRFT window");
        });
}
