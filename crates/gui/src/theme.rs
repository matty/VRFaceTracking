//! VRFT's look: a monochrome dark theme with one signal colour. Black, white
//! and greys carry everything; orange only ever means "look at this". The
//! app is always dark; pages use the theme's semantic tokens and
//! `vrft_gui_core::palette`, so they follow from here.
use gpui_kit::component::{Theme, ThemeConfig, ThemeMode, ThemeRegistry};
use gpui_kit::{App, SharedString, Window};
use std::borrow::Cow;
use std::rc::Rc;
use vrft_gui_core::palette::{MONO_FONT, SANS_FONT};

/// Geist and Geist Mono, embedded so the app looks the same on every PC.
/// They're under the SIL Open Font License, beside them in
/// `resources/fonts`.
const FONTS: [&[u8]; 6] = [
    include_bytes!("../resources/fonts/Geist-Regular.ttf"),
    include_bytes!("../resources/fonts/Geist-Medium.ttf"),
    include_bytes!("../resources/fonts/Geist-SemiBold.ttf"),
    include_bytes!("../resources/fonts/Geist-Bold.ttf"),
    include_bytes!("../resources/fonts/GeistMono-Regular.ttf"),
    include_bytes!("../resources/fonts/GeistMono-Medium.ttf"),
];

/// Replaces the default dark theme with VRFT's and applies it, whatever the
/// system's appearance.
pub fn install(window: &mut Window, cx: &mut App) {
    let fonts = FONTS.iter().map(|font| Cow::Borrowed(*font)).collect();
    if let Err(error) = cx.text_system().add_fonts(fonts) {
        // The system font stands in; everything still reads.
        log::warn!("Couldn't load the app's fonts: {error:#}");
    }
    let registry = ThemeRegistry::global(cx);
    let mut dark = (**registry.default_dark_theme()).clone();
    adjust(&mut dark);
    Theme::global_mut(cx).dark_theme = Rc::new(dark);
    Theme::change(ThemeMode::Dark, Some(window), cx);
}

fn adjust(config: &mut ThemeConfig) {
    let color = |value: &'static str| Some(SharedString::from(value));
    config.font_family = Some(SANS_FONT.into());
    config.mono_font_family = Some(MONO_FONT.into());
    config.radius = Some(8);
    config.radius_lg = Some(14);
    // Flat surfaces: depth comes from the ramp of greys, not shadows.
    config.shadow = Some(false);
    let colors = &mut config.colors;

    colors.background = color("#0a0a0b");
    colors.foreground = color("#fafafa");
    colors.muted = color("#161619");
    colors.muted_foreground = color("#858585");
    colors.border = color("#232327");
    colors.input = color("#2c2c31");
    colors.ring = color("#6a6a72");
    colors.selection = color("#3a3a41");
    colors.caret = color("#fafafa");
    colors.link = color("#fafafa");
    colors.link_hover = color("#ffffff");
    colors.link_active = color("#d4d4d4");
    colors.overlay = color("#000000b3");
    colors.window_border = color("#232327");

    // Cards, popovers and lists.
    colors.group_box = color("#111113");
    colors.group_box_foreground = color("#fafafa");
    colors.group_box_title_foreground = color("#b0b0b0");
    colors.popover = color("#141416");
    colors.popover_foreground = color("#fafafa");
    colors.list = color("#111113");
    colors.list_even = color("#111113");
    colors.list_head = color("#111113");
    colors.list_hover = color("#17171a");
    colors.list_active = color("#1f1f23");
    colors.list_active_border = color("#3a3a41");
    colors.accent = color("#1f1f23");
    colors.accent_foreground = color("#fafafa");
    colors.skeleton = color("#1a1a1d");
    colors.scrollbar = color("#00000000");
    colors.scrollbar_thumb = color("#2c2c31");
    colors.scrollbar_thumb_hover = color("#3a3a41");

    // The one white button per view is the next step.
    colors.primary = color("#fafafa");
    colors.primary_hover = color("#e2e2e2");
    colors.primary_active = color("#d4d4d4");
    colors.primary_foreground = color("#0a0a0b");
    colors.button_primary = color("#fafafa");
    colors.button_primary_hover = color("#e2e2e2");
    colors.button_primary_active = color("#d4d4d4");
    colors.button_primary_foreground = color("#0a0a0b");

    colors.secondary = color("#1a1a1d");
    colors.secondary_hover = color("#232327");
    colors.secondary_active = color("#2a2a2f");
    // Ghost buttons and other quiet controls read grey, and white on hover.
    colors.secondary_foreground = color("#b0b0b0");
    colors.button_secondary = color("#1a1a1d");
    colors.button_secondary_hover = color("#232327");
    colors.button_secondary_active = color("#2a2a2f");
    colors.button_secondary_foreground = color("#fafafa");
    colors.button = color("#1a1a1d");
    colors.button_hover = color("#232327");
    colors.button_active = color("#2a2a2f");
    colors.button_foreground = color("#fafafa");

    // State colours. Live is white and waiting is grey; the signal orange is
    // kept for what needs the player, including confirming something
    // destructive.
    colors.success = color("#fafafa");
    colors.success_hover = color("#e2e2e2");
    colors.success_active = color("#d4d4d4");
    colors.success_foreground = color("#0a0a0b");
    colors.button_success = color("#fafafa");
    colors.button_success_hover = color("#e2e2e2");
    colors.button_success_active = color("#d4d4d4");
    colors.button_success_foreground = color("#0a0a0b");
    colors.warning = color("#b0b0b0");
    colors.warning_hover = color("#c4c4c4");
    colors.warning_active = color("#9a9a9a");
    colors.warning_foreground = color("#0a0a0b");
    colors.info = color("#b0b0b0");
    colors.info_hover = color("#c4c4c4");
    colors.info_active = color("#9a9a9a");
    colors.info_foreground = color("#0a0a0b");
    colors.danger = color("#ff9447");
    colors.danger_hover = color("#ffa864");
    colors.danger_active = color("#f08030");
    colors.danger_foreground = color("#0a0a0b");
    colors.button_danger = color("#ff9447");
    colors.button_danger_hover = color("#ffa864");
    colors.button_danger_active = color("#f08030");
    colors.button_danger_foreground = color("#0a0a0b");

    // Charts and meters, as a ramp of greys from white.
    colors.chart_1 = color("#fafafa");
    colors.chart_2 = color("#d4d4d4");
    colors.chart_3 = color("#b0b0b0");
    colors.chart_4 = color("#858585");
    colors.chart_5 = color("#5f5f66");
    colors.progress_bar = color("#fafafa");
    colors.slider_bar = color("#fafafa");
    colors.slider_thumb = color("#fafafa");
    // The knob is black in both states, so it shows on the white "on" track;
    // the "off" track is a mid grey the knob still stands out from.
    colors.switch = color("#5f5f66");
    colors.switch_thumb = color("#0a0a0b");

    colors.sidebar = color("#0e0e10");
    colors.sidebar_border = color("#1e1e22");
    colors.sidebar_foreground = color("#a8a8a8");
    colors.sidebar_accent = color("#1f1f23");
    colors.sidebar_accent_foreground = color("#fafafa");
    colors.sidebar_primary = color("#fafafa");
    colors.sidebar_primary_foreground = color("#0a0a0b");
    colors.title_bar = color("#0a0a0b");
    colors.title_bar_border = color("#0a0a0b");
    colors.tab_bar = color("#0c0c0e");
    colors.tab_bar_segmented = color("#0c0c0e");
    colors.tab = color("#00000000");
    colors.tab_active = color("#26262b");
    colors.tab_active_foreground = color("#fafafa");
    colors.tab_foreground = color("#a8a8a8");
}
