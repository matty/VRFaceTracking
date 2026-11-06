//! Manage add-ons: every extension built into this app, to turn on or off and
//! to open. The tracking module, which every setup needs, is on Modules.
use crate::extension_switches::ExtensionSwitches;
use gpui_kit::component::v_flex;
use gpui_kit::{Context, Entity, IntoElement, ParentElement, Render, Styled, Window};
use rust_i18n::t;
use vrft_gui_core::widgets::PageHeader;

pub struct AddOnsPage {
    switches: Entity<ExtensionSwitches>,
}

impl AddOnsPage {
    pub fn new(switches: Entity<ExtensionSwitches>) -> Self {
        Self { switches }
    }
}

impl Render for AddOnsPage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_6()
            .child(PageHeader::new(t!("add_ons.title")).description(t!("add_ons.description")))
            .child(self.switches.clone())
    }
}
