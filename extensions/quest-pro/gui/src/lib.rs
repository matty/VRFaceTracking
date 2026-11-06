//! The Quest Pro add-on for the desktop app: tongue tracking and better eye
//! tracking. It adds the Headset, Mouth, Eyes and Training pages and its stage
//! in Home's signal path. The daemon side is `vrft-quest-pro-daemon`.
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
use gpui_kit::{App, AppContext as _, Entity, Window};
use headset::HeadsetPage;
use live::{CameraFeed, QuestProState};
use rust_i18n::t;
use summary::Tone;
use tongue::TongueTraining;
use vrft_gui_core::extension::{
    GuiExtension, GuiHost, PageEntry, PageId, PathSource, Reading, SetupOffer,
};
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
    brow_feed: Entity<CameraFeed>,
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
    let brow_feed = cx.new(|cx| CameraFeed::new(client.clone(), Camera::Brow, cx));
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
    let eyes = cx.new(|cx| {
        EyesPage::new(
            state.clone(),
            eye_feed.clone(),
            brow_feed.clone(),
            launcher,
            window,
            cx,
        )
    });
    let pages = vec![
        PageEntry {
            page: pages::HEADSET,
            view: headset.clone().into(),
        },
        PageEntry {
            page: pages::MOUTH,
            view: mouth.into(),
        },
        PageEntry {
            page: pages::EYES,
            view: eyes.into(),
        },
        PageEntry {
            page: pages::TRAINING,
            view: tongue.clone().into(),
        },
    ];
    Box::new(QuestProGui {
        state,
        mouth_feed,
        brow_feed,
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
        IconName::RectangleGoggles
    }

    fn setup_offer(&self) -> Option<SetupOffer> {
        Some(SetupOffer {
            question: t!("lib.setup_question").into(),
            features: vec![
                t!("lib.setup_tongue").into(),
                t!("lib.setup_eyes").into(),
                t!("lib.setup_previews").into(),
            ],
            requirements: Some(t!("lib.setup_requirements").into()),
            next_page: pages::HEADSET,
        })
    }

    fn pages(&self) -> &[PageEntry] {
        &self.pages
    }

    fn status(&self, cx: &App) -> Reading {
        let state = self.state.read(cx);
        summary::overall(state.status(), &state.rates())
    }

    fn page_tone(&self, page: PageId, cx: &App) -> Option<Tone> {
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
            icon: IconName::RectangleGoggles,
            label: vrft_quest_pro_protocol::NAME.into(),
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
        self.brow_feed
            .update(cx, |feed, cx| feed.set_watching(page == pages::EYES, cx));
    }

    fn wants_fast_status(&self, page: PageId) -> bool {
        // These pages show values that move with the wearer.
        page == pages::MOUTH || page == pages::TRAINING || page == pages::EYES
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
