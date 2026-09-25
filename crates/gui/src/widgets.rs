//! Small presentation pieces shared by the pages. Colors come from the theme's
//! semantic tokens, so both light and dark themes work.
use crate::summary::{Reading, Tone};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, Sizable as _, StyledExt as _};
use gpui_kit::{
    div, px, relative, AnyElement, App, Div, Hsla, IntoElement, ParentElement, RenderOnce,
    SharedString, Styled, Window,
};

pub fn tone_color(tone: Tone, cx: &App) -> Hsla {
    let theme = cx.theme();
    match tone {
        Tone::Good => theme.success,
        Tone::Waiting => theme.warning,
        Tone::Problem => theme.danger,
        Tone::Off => theme.muted_foreground,
    }
}

/// A colored dot. It always sits beside text that states the same thing, so
/// the meaning never depends on color alone.
#[derive(IntoElement)]
pub struct StatusDot {
    tone: Tone,
}

impl StatusDot {
    pub fn new(tone: Tone) -> Self {
        Self { tone }
    }
}

impl RenderOnce for StatusDot {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .flex_none()
            .size(px(8.))
            .rounded_full()
            .bg(tone_color(self.tone, cx))
    }
}

/// A horizontal bar filled to `fraction` (0..1). It follows live values
/// without animating, so it costs nothing between updates.
#[derive(IntoElement)]
pub struct Meter {
    fraction: f32,
    color: Hsla,
}

impl Meter {
    pub fn new(fraction: f32, color: Hsla) -> Self {
        Self {
            fraction: fraction.clamp(0., 1.),
            color,
        }
    }
}

impl RenderOnce for Meter {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .w_full()
            .h(px(6.))
            .rounded_full()
            .bg(self.color.opacity(0.2))
            .child(
                div()
                    .h_full()
                    .w(relative(self.fraction))
                    .rounded_full()
                    .bg(self.color),
            )
    }
}

/// A short outcome shown next to the action that caused it.
#[derive(IntoElement)]
pub struct Notice {
    tone: Tone,
    text: SharedString,
}

impl Notice {
    pub fn new(tone: Tone, text: impl Into<SharedString>) -> Self {
        Self {
            tone,
            text: text.into(),
        }
    }
}

impl RenderOnce for Notice {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        h_flex()
            .gap_2()
            .text_sm()
            .text_color(cx.theme().foreground)
            .child(StatusDot::new(self.tone))
            .child(self.text)
    }
}

/// A compact status label: a dot and a few words on a tinted background.
#[derive(IntoElement)]
pub struct StatusPill {
    tone: Tone,
    label: SharedString,
}

impl StatusPill {
    pub fn new(tone: Tone, label: impl Into<SharedString>) -> Self {
        Self {
            tone,
            label: label.into(),
        }
    }
}

impl RenderOnce for StatusPill {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = tone_color(self.tone, cx);
        h_flex()
            .flex_none()
            .gap_1p5()
            .h(px(22.))
            .px_2()
            .rounded_full()
            .bg(color.opacity(0.12))
            .text_xs()
            .font_medium()
            .text_color(cx.theme().foreground)
            .child(StatusDot::new(self.tone))
            .child(self.label)
    }
}

/// Page title with a one-line description and optional trailing content.
#[derive(IntoElement)]
pub struct PageHeader {
    title: SharedString,
    description: SharedString,
    trailing: Option<AnyElement>,
}

impl PageHeader {
    pub fn new(title: impl Into<SharedString>, description: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            description: description.into(),
            trailing: None,
        }
    }

    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing = Some(element.into_any_element());
        self
    }
}

impl RenderOnce for PageHeader {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        h_flex()
            .items_start()
            .justify_between()
            .gap_4()
            .child(
                v_flex()
                    .gap_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_xl()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(self.title),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.description),
                    ),
            )
            .children(self.trailing)
    }
}

/// A titled group of content on a page.
#[derive(IntoElement)]
pub struct Section {
    title: SharedString,
    children: Vec<AnyElement>,
}

impl Section {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            children: Vec::new(),
        }
    }
}

impl ParentElement for Section {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Section {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .font_medium()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.title),
            )
            .children(self.children)
    }
}

/// One part of the pipeline: what it is, its state, and a line of detail.
#[derive(IntoElement)]
pub struct StatTile {
    icon: Icon,
    label: SharedString,
    reading: Reading,
    action: Option<AnyElement>,
}

impl StatTile {
    pub fn new(icon: impl Into<Icon>, label: impl Into<SharedString>, reading: Reading) -> Self {
        Self {
            icon: icon.into(),
            label: label.into(),
            reading,
            action: None,
        }
    }

    /// A command that belongs to this part, shown in the tile's corner.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }
}

impl RenderOnce for StatTile {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let off = self.reading.tone == Tone::Off;
        let detail = if self.reading.detail.is_empty() {
            None
        } else {
            Some(self.reading.detail.clone())
        };
        v_flex()
            .min_w_0()
            .gap_3()
            .p_4()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.border)
            .bg(theme.group_box)
            .child(
                h_flex()
                    .gap_2()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap_2()
                            .min_w_0()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(self.icon.small())
                            .child(div().truncate().child(self.label)),
                    )
                    .child(h_flex().h(px(22.)).child(StatusDot::new(self.reading.tone))),
            )
            .child(
                v_flex()
                    .gap_0p5()
                    .min_w_0()
                    .child(
                        div()
                            .text_lg()
                            .font_semibold()
                            .truncate()
                            .text_color(if off {
                                theme.muted_foreground
                            } else {
                                theme.foreground
                            })
                            .child(self.reading.value),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_between()
                            .min_h(px(20.))
                            .child(
                                div()
                                    .min_w_0()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .line_clamp(2)
                                    .children(detail),
                            )
                            .children(self.action),
                    ),
            )
    }
}

/// A label and its value on one line.
pub fn info_row(label: &'static str, value: impl Into<SharedString>, cx: &App) -> Div {
    h_flex()
        .justify_between()
        .gap_4()
        .text_sm()
        .child(
            div()
                .flex_none()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(div().min_w_0().font_medium().truncate().child(value.into()))
}

/// A titled surface grouping related content.
#[derive(IntoElement)]
pub struct Panel {
    title: SharedString,
    children: Vec<AnyElement>,
}

impl Panel {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            children: Vec::new(),
        }
    }
}

impl ParentElement for Panel {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Panel {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .gap_3()
            .p_4()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.border)
            .bg(theme.group_box)
            .child(
                div()
                    .text_sm()
                    .font_medium()
                    .text_color(theme.muted_foreground)
                    .child(self.title),
            )
            .children(self.children)
    }
}
