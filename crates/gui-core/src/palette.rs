//! VRFT's colours and type, for what the theme's semantic tokens have no
//! slot for. One neutral ramp and one signal colour: shape carries state, so
//! nothing relies on colour alone, and orange only ever means "look at this".
use gpui_kit::{rgb, Hsla};

/// The interface's typeface, embedded by the app.
pub const SANS_FONT: &str = "Geist";
/// Numbers, addresses and small capital labels.
pub const MONO_FONT: &str = "Geist Mono";

/// The window behind every page.
pub fn page() -> Hsla {
    rgb(0x0a0a0b).into()
}

/// The navigation's background.
pub fn rail() -> Hsla {
    rgb(0x0e0e10).into()
}

/// Cards.
pub fn surface() -> Hsla {
    rgb(0x111113).into()
}

/// A card's inner blocks, such as a path stage or an option.
pub fn inset() -> Hsla {
    rgb(0x161619).into()
}

/// What stands out inside a card: the step to do now, the module in use.
pub fn raised() -> Hsla {
    rgb(0x18181b).into()
}

/// Behind previews, pads and text fields.
pub fn sunken() -> Hsla {
    rgb(0x0c0c0e).into()
}

/// A card's edge.
pub fn line() -> Hsla {
    rgb(0x212125).into()
}

/// Rules between rows inside a card.
pub fn line_soft() -> Hsla {
    rgb(0x1d1d21).into()
}

/// Edges that should read, such as a field's or a selected option's.
pub fn line_strong() -> Hsla {
    rgb(0x2c2c31).into()
}

/// The edge of what's selected or current.
pub fn line_focus() -> Hsla {
    rgb(0x6a6a72).into()
}

pub fn text() -> Hsla {
    rgb(0xfafafa).into()
}

/// Secondary text: descriptions and values beside a label.
pub fn text_2() -> Hsla {
    rgb(0xb0b0b0).into()
}

/// Captions and labels. The quietest text that still reads.
pub fn text_3() -> Hsla {
    rgb(0x858585).into()
}

/// Decoration only, such as a chevron or an idle ring; never words.
pub fn text_4() -> Hsla {
    rgb(0x5f5f66).into()
}

/// The signal colour: something needs the player.
pub fn signal() -> Hsla {
    rgb(0xff9447).into()
}

/// Words in the signal colour, a little lighter so they read.
pub fn signal_text() -> Hsla {
    rgb(0xffb27a).into()
}

/// Behind something that needs the player.
pub fn signal_bg() -> Hsla {
    rgb(0x1a120c).into()
}

/// The edge of something that needs the player.
pub fn signal_line() -> Hsla {
    rgb(0x5a3418).into()
}
