//! Small presentation pieces shared by the pages. Colours come from the
//! theme's semantic tokens and [`crate::palette`], never written out here.
use crate::nav::{open_page, PageId};
use crate::palette::{self, MONO_FONT};
use crate::summary::{Fix, Reading, Tone};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, Sizable as _, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, relative, AnyElement, App, ClipboardItem, Div, ElementId, Hsla,
    InteractiveElement as _, IntoElement, ParentElement, RenderOnce, SharedString,
    StatefulInteractiveElement as _, Styled, Window,
};
use rust_i18n::t;
use std::hash::{DefaultHasher, Hash as _, Hasher as _};

/// The colour of a state: white when it works, grey while waiting or off,
/// and the signal orange when it needs the player.
pub fn tone_color(tone: Tone, _cx: &App) -> Hsla {
    match tone {
        Tone::Good => palette::text(),
        Tone::Waiting => palette::text_2(),
        Tone::Problem => palette::signal(),
        Tone::Off => palette::text_3(),
    }
}

fn tone_icon(tone: Tone) -> IconName {
    match tone {
        Tone::Good => IconName::CircleCheck,
        Tone::Waiting => IconName::LoaderCircle,
        Tone::Problem => IconName::TriangleAlert,
        Tone::Off => IconName::Info,
    }
}

/// The button that does what a reading's fix says: opens the page with the
/// control, or the config file. The caller picks its look.
pub fn fix_button(id: impl Into<ElementId>, fix: Fix) -> Button {
    Button::new(id)
        .label(fix.label())
        .on_click(move |_, _, cx| match fix {
            Fix::Open(page) => open_page(page, cx),
            Fix::Config => {
                if let Some(path) = crate::paths::config_file() {
                    cx.open_with_system(&path);
                }
            }
        })
}

/// One button size for every labelled action: the component library's small
/// text at its regular height, so rows of buttons line up with body text.
pub trait ButtonExt {
    fn regular(self) -> Self;
    /// The size of a view's main action.
    fn prominent(self) -> Self;
}

impl ButtonExt for Button {
    fn regular(self) -> Self {
        self.small().h_8().px_3()
    }

    fn prominent(self) -> Self {
        self.h_10().px_4()
    }
}

/// A small capital label over a group, in the mono face: `SIGNAL PATH`,
/// `QUEST PRO ADD-ON`.
pub fn cap(text: impl Into<SharedString>) -> Div {
    let text: SharedString = text.into();
    div()
        .font_family(MONO_FONT)
        .text_size(px(11.))
        .font_medium()
        .text_color(palette::text_3())
        .child(SharedString::from(text.to_uppercase()))
}

/// The surface every card shares: a flat panel a step up from the page.
pub fn card(cx: &App) -> Div {
    div()
        .rounded(cx.theme().radius_lg)
        .border_1()
        .border_color(palette::line())
        .bg(palette::surface())
}

/// Numbers, addresses and versions, in the mono face so they line up.
pub fn mono(text: impl Into<SharedString>) -> Div {
    div().font_family(MONO_FONT).child(text.into())
}

/// A status mark whose shape says as much as its colour, so it reads
/// without colour: a solid dot when live, an open arc while working, a
/// hollow ring when off, and a warning sign when it needs the player. None
/// of them animates, so a page left open costs nothing between updates.
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
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let mark = div()
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .size(px(12.));
        match self.tone {
            Tone::Good => mark.child(
                div()
                    .size(px(10.))
                    .rounded_full()
                    .bg(palette::text().opacity(0.16))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(div().size(px(6.)).rounded_full().bg(palette::text())),
            ),
            Tone::Waiting => mark
                .text_color(palette::text_2())
                .child(Icon::new(IconName::LoaderCircle).size(px(12.))),
            Tone::Problem => mark
                .text_color(palette::signal())
                .child(Icon::new(IconName::TriangleAlert).size(px(12.))),
            Tone::Off => mark.child(
                div()
                    .size(px(8.))
                    .rounded_full()
                    .border_1()
                    .border_color(palette::text_4()),
            ),
        }
    }
}

/// A thin bar filled to `fraction` (0..1). It follows live values without
/// animating, so it costs nothing between updates.
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
            .h(px(4.))
            .rounded_full()
            .bg(palette::line_strong())
            .child(
                div()
                    .h_full()
                    .w(relative(self.fraction))
                    .rounded_full()
                    .bg(self.color),
            )
    }
}

/// An outcome or warning, shown next to what it's about. Only a problem is
/// coloured; everything else sits quietly on the card.
#[derive(IntoElement, Clone)]
pub struct Notice {
    tone: Tone,
    text: SharedString,
    /// The whole story, such as every cause of an error, for copying rather
    /// than reading in the box.
    details: Option<SharedString>,
}

impl Notice {
    pub fn new(tone: Tone, text: impl Into<SharedString>) -> Self {
        Self {
            tone,
            text: text.into(),
            details: None,
        }
    }

    /// A failure, as `prefix` and the error's outermost message, with its
    /// causes behind "Copy details" when it has any.
    pub fn error(prefix: &str, error: &anyhow::Error) -> Self {
        let brief = error.to_string();
        let full = format!("{error:#}");
        let text = if prefix.is_empty() {
            brief.clone()
        } else {
            t!("widgets.error", prefix = prefix, error = brief).into()
        };
        let notice = Self::new(Tone::Problem, text);
        if full == brief {
            notice
        } else {
            notice.details(full)
        }
    }

    pub fn details(mut self, details: impl Into<SharedString>) -> Self {
        self.details = Some(details.into());
        self
    }
}

impl RenderOnce for Notice {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = tone_color(self.tone, cx);
        let problem = self.tone == Tone::Problem;
        h_flex()
            .gap_2p5()
            .items_start()
            .px_3p5()
            .py_2p5()
            .rounded(px(10.))
            .border_1()
            .border_color(if problem {
                palette::signal_line()
            } else {
                palette::line_strong()
            })
            .bg(if problem {
                palette::signal_bg()
            } else {
                palette::inset()
            })
            .text_sm()
            .text_color(palette::text())
            .child(
                div()
                    .flex_none()
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .text_color(color)
                    .child(Icon::new(tone_icon(self.tone)).small()),
            )
            .child(div().flex_1().min_w_0().py(px(1.)).child(self.text))
            .children(self.details.map(|details| {
                let mut hasher = DefaultHasher::new();
                details.hash(&mut hasher);
                Button::new(SharedString::from(format!(
                    "copy-details-{:x}",
                    hasher.finish()
                )))
                .ghost()
                .xsmall()
                .icon(IconName::Copy)
                .label(t!("widgets.copy_details"))
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(details.to_string()))
                })
            }))
    }
}

/// A mark and a line of text: a state stated in words, without a box.
#[derive(IntoElement)]
pub struct StatusLine {
    tone: Tone,
    text: SharedString,
}

impl StatusLine {
    pub fn new(tone: Tone, text: impl Into<SharedString>) -> Self {
        Self {
            tone,
            text: text.into(),
        }
    }
}

impl RenderOnce for StatusLine {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        // Long lines wrap, with the mark beside their first line.
        h_flex()
            .gap_2()
            .items_start()
            .text_xs()
            .text_color(if self.tone == Tone::Problem {
                palette::signal_text()
            } else {
                palette::text_2()
            })
            .child(
                div()
                    .flex_none()
                    .h(px(16.))
                    .flex()
                    .items_center()
                    .child(StatusDot::new(self.tone)),
            )
            .child(div().flex_1().min_w_0().child(self.text))
    }
}

/// A compact status label: a mark and a few words in an outlined pill.
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
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let problem = self.tone == Tone::Problem;
        h_flex()
            .flex_none()
            .gap_2()
            .h(px(28.))
            .px_3()
            .rounded_full()
            .border_1()
            .border_color(if problem {
                palette::signal_line()
            } else {
                palette::line_strong()
            })
            .when(problem, |pill| pill.bg(palette::signal_bg()))
            .text_xs()
            .font_medium()
            .text_color(palette::text())
            .child(StatusDot::new(self.tone))
            .child(self.label)
    }
}

/// An icon on a small square, grey unless told otherwise.
#[derive(IntoElement)]
pub struct IconBadge {
    icon: IconName,
    color: Option<Hsla>,
    size: f32,
}

impl IconBadge {
    pub fn new(icon: IconName) -> Self {
        Self {
            icon,
            color: None,
            size: 32.,
        }
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }
}

impl RenderOnce for IconBadge {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let color = self.color.unwrap_or(palette::text_2());
        let icon = Icon::new(self.icon);
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(self.size))
            .rounded(px((self.size / 4.).round()))
            .bg(palette::raised())
            .border_1()
            .border_color(palette::line_strong())
            .text_color(color)
            .child(if self.size >= 40. {
                icon.large()
            } else {
                icon.small()
            })
    }
}

/// A page's title, with a line on what it's for and optional trailing
/// content such as a status pill.
#[derive(IntoElement)]
pub struct PageHeader {
    title: SharedString,
    description: Option<SharedString>,
    trailing: Option<AnyElement>,
}

impl PageHeader {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            description: None,
            trailing: None,
        }
    }

    /// One line under the title on what the page is for.
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing = Some(element.into_any_element());
        self
    }
}

impl RenderOnce for PageHeader {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        h_flex()
            .items_end()
            .justify_between()
            .gap_6()
            .child(
                v_flex()
                    .gap_1p5()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(26.))
                            .line_height(px(32.))
                            .font_semibold()
                            .truncate()
                            .text_color(palette::text())
                            .child(self.title),
                    )
                    .children(self.description.map(|description| {
                        div()
                            .text_sm()
                            .text_color(palette::text_2())
                            .child(description)
                    })),
            )
            .children(
                self.trailing
                    .map(|trailing| div().flex_none().child(trailing)),
            )
    }
}

/// A group of content on a page under a small capital label, with optional
/// quieter words on the right of the label.
#[derive(IntoElement)]
pub struct Section {
    title: SharedString,
    aside: Option<SharedString>,
    children: Vec<AnyElement>,
}

impl Section {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            aside: None,
            children: Vec::new(),
        }
    }

    /// A few words on the right of the label.
    pub fn aside(mut self, aside: impl Into<SharedString>) -> Self {
        self.aside = Some(aside.into());
        self
    }
}

impl ParentElement for Section {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Section {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .justify_between()
                    .gap_4()
                    .child(cap(self.title))
                    .children(
                        self.aside.map(|aside| {
                            div().text_xs().text_color(palette::text_3()).child(aside)
                        }),
                    ),
            )
            .children(self.children)
    }
}

/// Roughly how many characters of detail fit on a tile's line.
const TILE_DETAIL_FITS: usize = 34;

/// One part of what an extension adds, on Home: what it is, how it stands,
/// and a line of detail. The whole tile opens the part's page.
#[derive(IntoElement)]
pub struct StatTile {
    icon: IconName,
    label: SharedString,
    reading: Reading,
    page: Option<PageId>,
    visual: Option<AnyElement>,
    action: Option<AnyElement>,
}

impl StatTile {
    pub fn new(icon: IconName, label: impl Into<SharedString>, reading: Reading) -> Self {
        Self {
            icon,
            label: label.into(),
            reading,
            page: None,
            visual: None,
            action: None,
        }
    }

    /// The page the tile opens.
    pub fn page(mut self, page: PageId) -> Self {
        self.page = Some(page);
        self
    }

    /// A small live picture of the part, between the label and the value.
    pub fn visual(mut self, visual: impl IntoElement) -> Self {
        self.visual = Some(visual.into_any_element());
        self
    }

    /// A command beside the value, such as the fix for a problem.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }
}

impl RenderOnce for StatTile {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let tone = self.reading.tone;
        let detail = (!self.reading.detail.is_empty()).then(|| self.reading.detail.clone());
        let id = SharedString::from(format!("tile-{}", self.label));
        let detail_id = SharedString::from(format!("tile-detail-{}", self.label));
        let page = self.page;
        v_flex()
            .id(id)
            .min_w_0()
            .h(px(160.))
            .justify_between()
            .gap_3()
            .p_4()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(if tone == Tone::Problem {
                palette::signal_line()
            } else {
                palette::line()
            })
            .bg(palette::surface())
            .when_some(page, |tile, page| {
                tile.cursor_pointer()
                    .hover(|style| style.border_color(palette::line_focus()))
                    .on_click(move |_, _, cx| open_page(page, cx))
            })
            .child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(palette::text_2())
                    .child(Icon::new(self.icon).size(px(15.)))
                    .child(div().flex_1().min_w_0().truncate().child(self.label))
                    .when(page.is_some(), |row| {
                        row.child(
                            div()
                                .text_color(palette::text_4())
                                .child(Icon::new(IconName::ChevronRight).size(px(14.))),
                        )
                    }),
            )
            .children(self.visual.map(|visual| div().h(px(34.)).child(visual)))
            .child(
                h_flex()
                    .gap_2()
                    .items_end()
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_0p5()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .min_w_0()
                                    .when(matches!(tone, Tone::Problem | Tone::Waiting), |row| {
                                        row.child(StatusDot::new(tone))
                                    })
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(px(17.))
                                            .font_semibold()
                                            .text_color(match tone {
                                                Tone::Off => palette::text_2(),
                                                _ => palette::text(),
                                            })
                                            .child(self.reading.value),
                                    ),
                            )
                            .child(
                                div()
                                    .id(detail_id)
                                    .text_xs()
                                    .truncate()
                                    .text_color(palette::text_3())
                                    // One line can cut a long reason short;
                                    // the whole of it shows on hover.
                                    .when_some(
                                        detail
                                            .clone()
                                            .filter(|detail| detail.len() > TILE_DETAIL_FITS),
                                        |this, detail| {
                                            this.tooltip(move |window, cx| {
                                                Tooltip::new(detail.clone()).build(window, cx)
                                            })
                                        },
                                    )
                                    .children(detail),
                            ),
                    )
                    .children(self.action.map(|action| div().flex_none().child(action))),
            )
    }
}

/// A label and its value on one line.
pub fn info_row(label: &'static str, value: impl Into<SharedString>, _cx: &App) -> Div {
    h_flex()
        .justify_between()
        .gap_4()
        .text_size(px(13.))
        .child(div().flex_none().text_color(palette::text_3()).child(label))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(palette::text())
                .child(value.into()),
        )
}

/// A line of quieter explanation under the thing it explains.
pub fn hint(text: impl Into<SharedString>, _cx: &App) -> Div {
    div()
        .text_xs()
        .text_color(palette::text_3())
        .child(text.into())
}

/// A thin rule between the parts of a card.
pub fn divider(_cx: &App) -> Div {
    div().w_full().h(px(1.)).bg(palette::line_soft())
}

/// What stands in for a picture that isn't there: an icon, a title, a line
/// of detail, and optionally what to do about it.
#[derive(IntoElement)]
pub struct EmptyState {
    icon: IconName,
    title: SharedString,
    detail: SharedString,
    action: Option<AnyElement>,
}

impl EmptyState {
    pub fn new(
        icon: IconName,
        title: impl Into<SharedString>,
        detail: impl Into<SharedString>,
    ) -> Self {
        Self {
            icon,
            title: title.into(),
            detail: detail.into(),
            action: None,
        }
    }

    pub fn action(mut self, action: Option<AnyElement>) -> Self {
        self.action = action;
        self
    }
}

impl RenderOnce for EmptyState {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_1()
            .p_6()
            .text_center()
            .child(div().mb_3().child(IconBadge::new(self.icon).size(44.)))
            .child(
                div()
                    .text_base()
                    .font_semibold()
                    .text_color(palette::text())
                    .child(self.title),
            )
            .when(!self.detail.is_empty(), |this| {
                this.child(
                    div()
                        .max_w(px(420.))
                        .text_sm()
                        .text_color(palette::text_2())
                        .child(self.detail),
                )
            })
            .children(self.action.map(|action| div().mt_4().child(action)))
    }
}

/// A titled card grouping related content, with an optional line under its
/// title and content on the right of it.
#[derive(IntoElement)]
pub struct Panel {
    title: SharedString,
    description: Option<SharedString>,
    trailing: Option<AnyElement>,
    children: Vec<AnyElement>,
}

impl Panel {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            description: None,
            trailing: None,
            children: Vec::new(),
        }
    }

    /// A line under the title on what the card is for.
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing = Some(element.into_any_element());
        self
    }
}

impl ParentElement for Panel {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Panel {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        card(cx)
            .flex()
            .flex_col()
            .min_w_0()
            .gap_4()
            .px(px(18.))
            .pt_4()
            .pb(px(18.))
            .child(
                h_flex()
                    .gap_3()
                    .items_start()
                    .justify_between()
                    .child(
                        v_flex()
                            .gap_0p5()
                            .min_w_0()
                            .child(
                                div()
                                    .min_h(px(24.))
                                    .flex()
                                    .items_center()
                                    .text_size(px(14.5))
                                    .font_semibold()
                                    .text_color(palette::text())
                                    .child(self.title),
                            )
                            .children(self.description.map(|description| {
                                div()
                                    .text_xs()
                                    .text_color(palette::text_3())
                                    .child(description)
                            })),
                    )
                    .children(
                        self.trailing
                            .map(|trailing| div().flex_none().child(trailing)),
                    ),
            )
            .children(self.children)
    }
}
