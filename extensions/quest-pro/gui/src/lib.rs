//! The Quest Pro add-on for the desktop app: tongue tracking and better eye
//! tracking. It adds the Headset, Mouth, Eyes and Training pages, its
//! tiles on Home and its stage in Home's signal path. The daemon side is
//! `vrft-quest-pro-daemon`.
mod adb;
mod camera;
mod daemon;
mod eyes;
mod headset;
mod headset_app;
mod live;
mod prompts;
mod speech;
mod summary;
mod tongue;

use camera::MouthPage;
use daemon::Camera;
use eyes::EyesPage;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::{h_flex, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    canvas, div, point, px, size, App, AppContext as _, Bounds, Entity, IntoElement, ParentElement,
    PathBuilder, Pixels, Styled, Window,
};
use headset::HeadsetPage;
use live::{CameraFeed, QuestProState};
use rust_i18n::t;
use summary::{Tone, TongueReading, TongueState};
use tongue::TongueTraining;
use vrft_gui_core::extension::{GuiExtension, GuiHost, PageEntry, PageId, PathSource, StatTile};
use vrft_gui_core::palette::{self, MONO_FONT};
use vrft_gui_core::widgets::fix_button;
// Text in `locales/`, in the locale the app chose. Keys missing from a
// language fall back to English.
rust_i18n::i18n!("locales", fallback = "en");

/// The extension's id, shared with its daemon half.
pub use vrft_quest_pro_protocol::ID;

/// The extension's pages.
pub mod pages {
    use rust_i18n::t;
    use vrft_gui_core::extension::PageId;

    pub const HEADSET: PageId = PageId::translated("quest-pro/headset", || t!("lib.page_headset"));
    pub const MOUTH: PageId = PageId::translated("quest-pro/mouth", || t!("lib.page_mouth"));
    pub const EYES: PageId = PageId::translated("quest-pro/eyes", || t!("lib.page_eyes"));
    pub const TRAINING: PageId =
        PageId::translated("quest-pro/training", || t!("lib.page_training"));
}

pub struct QuestProGui {
    state: Entity<QuestProState>,
    mouth_feed: Entity<CameraFeed>,
    eye_feed: Entity<CameraFeed>,
    headset: Entity<HeadsetPage>,
    tongue: Entity<TongueTraining>,
    pages: Vec<PageEntry>,
}

/// Builds the extension. Its [`vrft_gui_core::extension::Factory`].
pub fn create(host: &GuiHost, window: &mut Window, cx: &mut App) -> Box<dyn GuiExtension> {
    tongue::bind_keys(cx);
    let state = cx.new(|cx| QuestProState::new(host.daemon.clone(), cx));
    let client = state.read(cx).client();
    let mouth_feed = cx.new(|cx| CameraFeed::new(client.clone(), Camera::Mouth, cx));
    let eye_feed = cx.new(|cx| CameraFeed::new(client, Camera::Eyes, cx));
    let launcher = host.launcher.clone();
    let headset = cx.new(|cx| HeadsetPage::new(state.clone(), window, cx));
    let tongue = cx.new(|cx| {
        TongueTraining::new(
            state.clone(),
            mouth_feed.clone(),
            launcher.clone(),
            window,
            cx,
        )
    });
    let mouth = cx.new(|cx| {
        MouthPage::new(
            state.clone(),
            mouth_feed.clone(),
            launcher.clone(),
            window,
            cx,
        )
    });
    let eyes = cx.new(|cx| EyesPage::new(state.clone(), eye_feed.clone(), launcher, cx));
    let pages = vec![
        PageEntry {
            page: pages::HEADSET,
            icon: IconName::Glasses,
            view: headset.clone().into(),
        },
        PageEntry {
            page: pages::MOUTH,
            icon: IconName::FaceGrinning,
            view: mouth.into(),
        },
        PageEntry {
            page: pages::EYES,
            icon: IconName::ScanEye,
            view: eyes.into(),
        },
        PageEntry {
            page: pages::TRAINING,
            icon: IconName::Sparkles,
            view: tongue.clone().into(),
        },
    ];
    Box::new(QuestProGui {
        state,
        mouth_feed,
        eye_feed,
        headset,
        tongue,
        pages,
    })
}

impl GuiExtension for QuestProGui {
    fn id(&self) -> &'static str {
        ID
    }

    fn name(&self) -> &'static str {
        vrft_quest_pro_protocol::NAME
    }

    fn description(&self) -> gpui_kit::SharedString {
        t!("lib.description").into()
    }

    fn icon(&self) -> IconName {
        IconName::Glasses
    }

    fn pages(&self) -> &[PageEntry] {
        &self.pages
    }

    fn nav_tone(&self, page: PageId, cx: &App) -> Option<Tone> {
        let state = self.state.read(cx);
        let status = state.status();
        let tone = match page {
            page if page == pages::HEADSET => match summary::headset(status).tone {
                // A headset that simply isn't connected yet isn't worth a dot.
                Tone::Waiting => Tone::Off,
                tone => tone,
            },
            page if page == pages::MOUTH => summary::mouth_cameras(status, &state.rates()).tone,
            page if page == pages::EYES => summary::eyes(status).tone,
            page if page == pages::TRAINING => summary::tongue_model(status).tone,
            _ => return None,
        };
        Some(tone)
    }

    fn home_tiles(&self, cx: &App) -> Vec<StatTile> {
        let state = self.state.read(cx);
        let status = state.status();
        let rates = state.rates();
        let eyes = summary::eyes(status);
        // With no eye data the fix is on another page than the eyes'.
        let eyes_fix = eyes
            .fix
            .filter(|fix| *fix != summary::Fix::Open(pages::EYES))
            .map(|fix| fix_button("fix-eyes", fix).ghost().xsmall());
        let gaze = status.and_then(|status| status.eye_output_deg);
        let tongue = status
            .map(|status| summary::tongue(status, rates.tracking()))
            .filter(|tongue| tongue.state == TongueState::Out);
        vec![
            StatTile::new(
                IconName::Glasses,
                t!("lib.tile_headset"),
                summary::headset(status),
            )
            .page(pages::HEADSET),
            StatTile::new(
                IconName::FaceGrinning,
                t!("lib.tile_mouth"),
                summary::mouth_cameras(status, &rates),
            )
            .page(pages::MOUTH),
            StatTile::new(IconName::ScanEye, t!("lib.tile_eyes"), eyes)
                .page(pages::EYES)
                .when_some(gaze, |tile, gaze| tile.visual(GazeGlyph { gaze }))
                .when_some(eyes_fix, |tile, fix| tile.action(fix)),
            StatTile::new(
                IconName::Sparkles,
                t!("lib.tile_training"),
                summary::tongue_model(status),
            )
            .page(pages::TRAINING)
            .when_some(tongue, |tile, tongue| tile.visual(TongueGlyph { tongue })),
        ]
    }

    fn path_source(&self, cx: &App) -> Option<PathSource> {
        let state = self.state.read(cx);
        let status = state.status();
        let rates = state.rates();
        let headset = summary::headset(status);
        let cameras = summary::mouth_cameras(status, &rates);
        let tone = if headset.tone == Tone::Problem {
            Tone::Problem
        } else {
            cameras.tone
        };
        Some(PathSource {
            icon: IconName::Glasses,
            label: t!("lib.add_on", name = vrft_quest_pro_protocol::NAME).into(),
            value: rates
                .camera_fps
                .filter(|_| cameras.tone == Tone::Good)
                .map(summary::fps),
            tone,
            page: pages::HEADSET,
        })
    }

    fn page_changed(&mut self, page: PageId, cx: &mut App) {
        self.headset.update(cx, |headset, cx| {
            headset.set_watching(page == pages::HEADSET, cx)
        });
        // Recording a tongue shows the mouth cameras too.
        self.mouth_feed.update(cx, |feed, cx| {
            feed.set_watching(page == pages::MOUTH || page == pages::TRAINING, cx)
        });
        self.tongue.update(cx, |tongue, cx| {
            tongue.set_watching(page == pages::TRAINING, cx)
        });
        self.eye_feed
            .update(cx, |feed, cx| feed.set_watching(page == pages::EYES, cx));
    }

    fn wants_fast_status(&self, page: PageId) -> bool {
        // These pages show values that move with the wearer.
        page == pages::MOUTH || page == pages::TRAINING || page == pages::EYES
    }
}

/// Both eyes' gaze from above, as on the Eyes page, small enough for a tile:
/// the left eye's ray solid, the right eye's dashed.
#[derive(IntoElement)]
struct GazeGlyph {
    gaze: [[f32; 2]; 2],
}

impl gpui_kit::RenderOnce for GazeGlyph {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let gaze = self.gaze;
        canvas(
            |_, _, _| {},
            move |bounds: Bounds<Pixels>, _, window, _| {
                let origin = bounds.origin;
                let middle = bounds.size.width.as_f32() / 2.;
                let base = bounds.size.height.as_f32() - 4.;
                let at = |x: f32, y: f32| point(origin.x + px(x), origin.y + px(y));
                for (index, offset) in [-13.0f32, 13.].into_iter().enumerate() {
                    let x = middle + offset;
                    // Yaw is positive to the right.
                    let yaw = gaze[index][0].to_radians();
                    let length = 28.;
                    let mut ray = if index == 0 {
                        PathBuilder::stroke(px(1.25))
                    } else {
                        PathBuilder::stroke(px(1.25)).dash_array(&[px(3.), px(3.)])
                    };
                    ray.move_to(at(x, base));
                    ray.line_to(at(x + yaw.sin() * length, base - yaw.cos() * length));
                    if let Ok(path) = ray.build() {
                        window.paint_path(path, palette::text());
                    }
                    let dot = Bounds::centered_at(at(x, base), size(px(5.), px(5.)));
                    window.paint_quad(gpui_kit::fill(dot, palette::text()).corner_radii(px(2.5)));
                }
            },
        )
        .size_full()
    }
}

/// Where the tongue is, as a dot in a ring, and how far out.
#[derive(IntoElement)]
struct TongueGlyph {
    tongue: TongueReading,
}

impl gpui_kit::RenderOnce for TongueGlyph {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let tongue = self.tongue;
        let ring = 32.;
        let dot = 8.;
        let reach = (ring - dot) / 2. - 2.;
        h_flex()
            .h_full()
            .gap_2p5()
            .child(
                div()
                    .relative()
                    .flex_none()
                    .size(px(ring))
                    .rounded_full()
                    .border_1()
                    .border_color(palette::line_focus())
                    .child(
                        div()
                            .absolute()
                            .left(px(ring / 2. - dot / 2. - 1. + tongue.horizontal * reach))
                            .top(px(ring / 2. - dot / 2. - 1. - tongue.vertical * reach))
                            .size(px(dot))
                            .rounded_full()
                            .bg(palette::text()),
                    ),
            )
            .child(
                div()
                    .font_family(MONO_FONT)
                    .text_size(px(11.))
                    .text_color(palette::text_3())
                    .child(t!(
                        "lib.tongue_out",
                        percent = format!("{:.0}", tongue.out * 100.)
                    )),
            )
    }
}

#[cfg(test)]
mod i18n_tests {
    #[test]
    fn every_key_has_english_text() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let missing: Vec<_> = vrft_gui_core::translation_keys(&src)
            .into_iter()
            .filter(|key| super::_rust_i18n_try_translate("en", key).is_none())
            .collect();
        assert!(missing.is_empty(), "no English text for {missing:?}");
    }
}
