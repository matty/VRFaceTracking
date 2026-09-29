//! The contract a desktop app extension builds against. It pairs with the
//! daemon extension of the same id, whose status and routes it reads through
//! [`DaemonState`] and [`DaemonClient`](crate::client::DaemonClient), typed by
//! the extension's own protocol crate.
use crate::launcher::Launcher;
use crate::live::DaemonState;
pub use crate::nav::{open_page, PageId};
pub use crate::summary::{Fix, Reading, Tone};
pub use crate::widgets::StatTile;
use gpui_kit::assets::IconName;
use gpui_kit::{AnyView, App, Entity, SharedString, Window};

/// What the app gives an extension when it builds its pages.
pub struct GuiHost {
    pub daemon: Entity<DaemonState>,
    pub launcher: Entity<Launcher>,
}

/// A source an extension adds to Home's signal path, beside the tracking
/// module: data that joins the module's before smoothing and output.
pub struct PathSource {
    pub icon: IconName,
    /// What to call it, such as `Quest Pro add-on`.
    pub label: String,
    /// A rate in a few characters, such as `24 fps`, while it has one.
    pub value: Option<String>,
    pub tone: Tone,
    /// The page that manages it; the stage opens it.
    pub page: PageId,
}

/// One page an extension adds to the navigation.
pub struct PageEntry {
    pub page: PageId,
    pub icon: IconName,
    pub view: AnyView,
}

pub trait GuiExtension {
    /// The daemon extension's id, such as `quest-pro`.
    fn id(&self) -> &'static str;

    /// The navigation group's title and the Home section's, such as
    /// `Quest Pro`.
    fn name(&self) -> &'static str;

    /// One line on what turning the extension on adds.
    fn description(&self) -> SharedString;

    /// The extension's mark, on Modules beside its switch.
    fn icon(&self) -> IconName {
        IconName::Puzzle
    }

    /// The extension's pages, in navigation order.
    fn pages(&self) -> &[PageEntry];

    /// The dot beside `page` in the navigation. Only a part that's doing
    /// something, or needs attention, gets one.
    fn nav_tone(&self, _page: PageId, _cx: &App) -> Option<Tone> {
        None
    }

    /// Tiles for the extension's section on Home.
    fn home_tiles(&self, _cx: &App) -> Vec<StatTile> {
        Vec::new()
    }

    /// The extension's stage in Home's signal path, when it feeds tracking.
    fn path_source(&self, _cx: &App) -> Option<PathSource> {
        None
    }

    /// Called when the window shows `page`, which may be another
    /// extension's, so the extension only works while its pages show.
    fn page_changed(&mut self, _page: PageId, _cx: &mut App) {}

    /// Whether the daemon's status should be polled quickly while `page`
    /// shows, for values that move with the wearer.
    fn wants_fast_status(&self, _page: PageId) -> bool {
        false
    }
}

/// Builds an extension's pages and state.
pub type Factory = fn(&GuiHost, &mut Window, &mut App) -> Box<dyn GuiExtension>;
