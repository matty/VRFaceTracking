//! Which page the window shows. Any page, including an extension's, opens
//! another with [`open_page`], such as to point at the next step.
use gpui_kit::{App, AppContext as _, Context, Entity, Global, SharedString};
use rust_i18n::t;
use std::borrow::Cow;
use std::hash::{Hash, Hasher};

/// A page in the navigation. Extensions namespace their ids, such as
/// `quest-pro/headset`. Pages are the same page when their ids are; compare
/// them with `==`, as a name that follows the language can't be matched on.
#[derive(Debug, Clone, Copy)]
pub struct PageId {
    pub id: &'static str,
    label: Label,
}

/// A page's name in the navigation and on buttons that open it.
#[derive(Debug, Clone, Copy)]
enum Label {
    Text(&'static str),
    /// Looked up each time, so it follows the app's language.
    Translated(fn() -> Cow<'static, str>),
}

impl PageId {
    pub const HOME: PageId = PageId::translated("home", || t!("nav.home"));
    pub const SETTINGS: PageId = PageId::translated("settings", || t!("nav.settings"));
    pub const MODULES: PageId = PageId::translated("modules", || t!("nav.modules"));
    pub const TRACKING: PageId = PageId::translated("tracking", || t!("nav.tracking"));

    /// A page named `label` in every language.
    pub const fn new(id: &'static str, label: &'static str) -> Self {
        Self {
            id,
            label: Label::Text(label),
        }
    }

    /// A page whose name `label` gives in the app's language, such as
    /// `|| t!("nav.home")`.
    pub const fn translated(id: &'static str, label: fn() -> Cow<'static, str>) -> Self {
        Self {
            id,
            label: Label::Translated(label),
        }
    }

    /// The page's name in the navigation and on buttons that open it.
    pub fn label(&self) -> SharedString {
        match self.label {
            Label::Text(text) => text.into(),
            Label::Translated(label) => label().into(),
        }
    }
}

impl PartialEq for PageId {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for PageId {}

impl Hash for PageId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

/// The page on show. The window observes it.
pub struct Navigation {
    current: PageId,
}

impl Navigation {
    pub fn current(&self) -> PageId {
        self.current
    }

    pub fn open(&mut self, page: PageId, cx: &mut Context<Self>) {
        if self.current != page {
            self.current = page;
            cx.notify();
        }
    }
}

struct GlobalNavigation(Entity<Navigation>);

impl Global for GlobalNavigation {}

/// Creates the app's navigation, on `Home`.
pub fn install(cx: &mut App) -> Entity<Navigation> {
    let navigation = cx.new(|_| Navigation {
        current: PageId::HOME,
    });
    cx.set_global(GlobalNavigation(navigation.clone()));
    navigation
}

/// Shows `page`.
pub fn open_page(page: PageId, cx: &mut App) {
    if let Some(GlobalNavigation(navigation)) = cx.try_global::<GlobalNavigation>() {
        navigation
            .clone()
            .update(cx, |navigation, cx| navigation.open(page, cx));
    }
}
