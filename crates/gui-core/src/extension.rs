//! The contract a desktop app extension builds against. It pairs with the
//! daemon extension of the same id, whose status and routes it reads through
//! [`DaemonState`] and [`DaemonClient`](crate::client::DaemonClient), typed by
//! the extension's own protocol crate.
use crate::launcher::Launcher;
use crate::live::DaemonState;
pub use crate::nav::{open_page, PageId};
pub use crate::summary::{Fix, Reading, Tone};
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
    /// What to call it, such as `Quest Pro`. Home marks it as an add-on.
    pub label: String,
    /// A rate in a few characters, such as `24 fps`, while it has one.
    pub value: Option<String>,
    /// A few words under the label on why it isn't working.
    pub detail: Option<String>,
    /// A button on the stage: its label and the page it opens.
    pub action: Option<(String, PageId)>,
    pub tone: Tone,
    /// The page that manages it; the stage opens it.
    pub page: PageId,
}

/// How the app's first-launch setup offers an extension for hardware only
/// some people have: a question to answer yes or no, and what yes turns on.
pub struct SetupOffer {
    /// Such as `Do you have a Quest Pro?`.
    pub question: SharedString,
    /// What turning the extension on adds, a line each.
    pub features: Vec<SharedString>,
    /// What it needs beyond the hardware, if anything.
    pub requirements: Option<SharedString>,
    /// The page to open when setup has turned it on, for its next step.
    pub next_page: PageId,
}

/// One page of an extension, as a tab under its name.
pub struct PageEntry {
    pub page: PageId,
    pub view: AnyView,
}

pub trait GuiExtension {
    /// The daemon extension's id, such as `quest-pro`.
    fn id(&self) -> &'static str;

    /// What the navigation and the extension's own area call it, such as
    /// `Quest Pro`.
    fn name(&self) -> &'static str;

    /// One line on what turning the extension on adds.
    fn description(&self) -> SharedString;

    /// The extension's mark, in the navigation and at the top of its pages.
    fn icon(&self) -> IconName {
        IconName::Puzzle
    }

    /// What the first-launch setup asks to turn the extension on. `None`
    /// leaves it out of setup.
    fn setup_offer(&self) -> Option<SetupOffer> {
        None
    }

    /// The extension's pages, in tab order. The first opens by default.
    fn pages(&self) -> &[PageEntry];

    /// How the extension stands as a whole, in one reading: beside its name
    /// in the navigation and at the top of its pages. When several parts have
    /// something to say, that's the one to act on first.
    fn status(&self, _cx: &App) -> Reading {
        Reading::new(Tone::Off, "", "")
    }

    /// How far through first-time setup it is, as steps done and steps in
    /// all, while that's what it's doing.
    fn setup_progress(&self, _cx: &App) -> Option<(usize, usize)> {
        None
    }

    /// The one page that shows for now, when the others have nothing to show,
    /// such as during setup. Its tabs are hidden meanwhile.
    fn only_page(&self, _cx: &App) -> Option<PageId> {
        None
    }

    /// How `page` stands. Its tab is marked only when it needs attention.
    fn page_tone(&self, _page: PageId, _cx: &App) -> Option<Tone> {
        None
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
