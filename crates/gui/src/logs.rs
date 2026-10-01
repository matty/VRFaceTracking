//! Logs: what tracking and this app have recorded, read live as they grow,
//! for finding out what went wrong. Filters narrow them to the problems or
//! to some text, a line opens in full, and a report gathers both logs,
//! `config.json` and how things stand into one file to send to whoever is
//! helping. Detailed logging records more until the app closes.
//!
//! The app's own log is set up here too, by [`init_logging`]: a release has
//! no console, so it goes to `vrft_app.log`, and to the console as well in a
//! development build.
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    h_flex, v_flex, Disableable as _, Icon, Selectable as _, Sizable as _, StyledExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, uniform_list, AnyElement, AppContext as _, ClipboardItem, Context, Div, Entity, Hsla,
    InteractiveElement as _, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement as _, Styled, Subscription, Task, UniformListScrollHandle, Window,
};
use log::Level;
use rust_i18n::t;
use std::fs::File;
use std::io::Write;
use std::ops::Range;
use std::path::PathBuf;
use std::time::Duration;
use vrft_gui_core::launcher::Launcher;
use vrft_gui_core::live::DaemonState;
use vrft_gui_core::logs::{self, LogLine, LogTail, ReportFacts, TailChange};
use vrft_gui_core::palette::{self, MONO_FONT};
use vrft_gui_core::summary::{Connection, Tone};
use vrft_gui_core::widgets::{card, hint, ButtonExt as _, Notice, PageHeader};

/// How often the log on show is read again.
const POLL_INTERVAL: Duration = Duration::from_millis(500);
const ROW_HEIGHT: f32 = 20.;
/// The list never gets shorter than this, however small the window.
const MIN_LIST_HEIGHT: f32 = 260.;
/// What the page around the list takes up, less the list.
const AROUND_LIST: f32 = 330.;
/// Below this content width the target column is left out.
const TARGET_MIN_WIDTH: f32 = 760.;

/// Sets up the app's log: `RUST_LOG` when it's set, otherwise its own
/// default, which [`logs::set_detailed`] raises. It's written to
/// `vrft_app.log`, with the last run's kept beside it, and in a development
/// build to the console too.
pub fn init_logging() {
    let file = logs::app_log().and_then(|path| {
        logs::keep_previous(&path);
        File::create(path).ok()
    });
    let user_filter = std::env::var("RUST_LOG").ok();
    let mut builder = env_logger::Builder::new();
    builder
        .parse_filters(user_filter.as_deref().unwrap_or(logs::DETAILED_FILTER))
        .write_style(env_logger::WriteStyle::Never)
        .target(env_logger::Target::Pipe(Box::new(Tee {
            file,
            console: cfg!(debug_assertions),
        })));
    if builder.try_init().is_err() {
        return;
    }
    if user_filter.is_none() {
        log::set_max_level(log::LevelFilter::Info);
    }
    log::info!(
        "Starting {} {} on {} {}",
        crate::updates::app_name(),
        crate::updates::VERSION,
        std::env::consts::OS,
        std::env::consts::ARCH
    );
}

/// Writes the app's log to its file and, in a development build, the
/// console.
struct Tee {
    file: Option<File>,
    console: bool,
}

impl Write for Tee {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if let Some(file) = &mut self.file {
            // Logging must never fail the app, so a full disk is ignored.
            let _ = file.write_all(bytes);
        }
        if self.console {
            let _ = std::io::stderr().write_all(bytes);
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(file) = &mut self.file {
            let _ = file.flush();
        }
        Ok(())
    }
}

/// Which log the page shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Tracking,
    /// Tracking's run before this one, such as one that crashed.
    Previous,
    App,
}

impl Source {
    const ALL: [Source; 3] = [Source::Tracking, Source::Previous, Source::App];

    fn index(self) -> usize {
        self as usize
    }

    fn label(self) -> SharedString {
        match self {
            Source::Tracking => t!("logs.source_tracking"),
            Source::Previous => t!("logs.source_previous"),
            Source::App => t!("logs.source_app"),
        }
        .into()
    }

    fn path(self) -> Option<PathBuf> {
        match self {
            Source::Tracking => logs::daemon_log(),
            Source::Previous => logs::daemon_log().map(|path| logs::previous(&path)),
            Source::App => logs::app_log(),
        }
    }

    /// What the list says when the log isn't there.
    fn missing(self) -> SharedString {
        match self {
            Source::Tracking => t!("logs.missing_tracking"),
            Source::Previous => t!("logs.missing_previous"),
            Source::App => t!("logs.missing_app"),
        }
        .into()
    }
}

/// Which lines show, by how severe they are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Severity {
    All,
    Warnings,
    Errors,
}

impl Severity {
    const ALL: [Severity; 3] = [Severity::All, Severity::Warnings, Severity::Errors];

    fn admits(self, line: &LogLine) -> bool {
        match self {
            Severity::All => true,
            Severity::Warnings => line.at_least(Level::Warn),
            Severity::Errors => line.at_least(Level::Error),
        }
    }
}

/// How many records of each problem level a log has.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Counts {
    errors: usize,
    warnings: usize,
}

impl Counts {
    fn of(lines: &[LogLine]) -> Self {
        let mut counts = Self::default();
        for line in lines.iter().filter(|line| !line.continuation) {
            match line.level {
                Some(Level::Error) => counts.errors += 1,
                Some(Level::Warn) => counts.warnings += 1,
                _ => {}
            }
        }
        counts
    }
}

/// Whether `line` has `query`, already lower case, anywhere in it.
fn matches(line: &LogLine, query: &str) -> bool {
    query.is_empty() || line.raw.to_lowercase().contains(query)
}

/// The record `lines[index]` belongs to: its first line and those carrying
/// it on.
fn record_around(lines: &[LogLine], index: usize) -> Range<usize> {
    let mut start = index;
    while start > 0 && lines[start].continuation {
        start -= 1;
    }
    let mut end = index + 1;
    while end < lines.len() && lines[end].continuation {
        end += 1;
    }
    start..end
}

/// The colours of a line's level and its message.
fn level_colors(level: Option<Level>) -> (Hsla, Hsla) {
    match level {
        Some(Level::Error) => (palette::signal(), palette::signal_text()),
        Some(Level::Warn) => (palette::text(), palette::text()),
        Some(Level::Info) | None => (palette::text_3(), palette::text_2()),
        Some(Level::Debug | Level::Trace) => (palette::text_4(), palette::text_3()),
    }
}

pub struct LogsPage {
    daemon: Entity<DaemonState>,
    launcher: Entity<Launcher>,
    source: Source,
    /// Each source's log, once it's been shown.
    tails: [Option<LogTail>; 3],
    severity: Severity,
    search: Entity<InputState>,
    /// The search as last applied, lower case.
    query: String,
    /// The `seq` of each line that passes the filters.
    visible: Vec<u64>,
    /// The last `seq` the filters have seen.
    filtered_to: Option<u64>,
    counts: Counts,
    /// The line opened in full, by its `seq`.
    selected: Option<u64>,
    scroll: UniformListScrollHandle,
    /// Minutes from UTC, which the logs use, to this PC's time.
    offset_minutes: i32,
    watching: bool,
    report: Option<Notice>,
    reporting: bool,
    _poll: Task<()>,
    _report: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl LogsPage {
    pub fn new(
        daemon: Entity<DaemonState>,
        launcher: Entity<Launcher>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("logs.search_placeholder")));
        let poll = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(POLL_INTERVAL).await;
            let Ok(()) = this.update(cx, |page, cx| {
                if page.watching {
                    page.read(cx);
                }
            }) else {
                break;
            };
        });
        let subscriptions = vec![
            cx.observe(&search, |page, _, cx| page.search_changed(cx)),
            // Says whether tracking was started here, so logs here.
            cx.observe(&launcher, |_, _, cx| cx.notify()),
        ];
        Self {
            daemon,
            launcher,
            source: Source::Tracking,
            tails: [None, None, None],
            severity: Severity::All,
            search,
            query: String::new(),
            visible: Vec::new(),
            filtered_to: None,
            counts: Counts::default(),
            selected: None,
            scroll: UniformListScrollHandle::new(),
            offset_minutes: logs::local_offset_minutes(),
            watching: false,
            report: None,
            reporting: false,
            _poll: poll,
            _report: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn set_watching(&mut self, watching: bool, cx: &mut Context<Self>) {
        self.watching = watching;
        if watching {
            self.offset_minutes = logs::local_offset_minutes();
            self.read(cx);
            self.scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    fn tail(&self) -> Option<&LogTail> {
        self.tails[self.source.index()].as_ref()
    }

    /// Reads what's been added to the log on show, and follows it when the
    /// list was at its end.
    fn read(&mut self, cx: &mut Context<Self>) {
        let source = self.source;
        let slot = &mut self.tails[source.index()];
        if slot.is_none() {
            let Some(path) = source.path() else {
                return;
            };
            *slot = Some(LogTail::new(path));
        }
        let Some(tail) = slot.as_mut() else {
            return;
        };
        let following = self.scroll.is_scrolled_to_end().unwrap_or(true);
        match tail.poll() {
            TailChange::None => return,
            TailChange::Grew => self.filter_new(),
            TailChange::Replaced => self.refilter(),
        }
        if following {
            self.scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    /// Filters every line again, such as for another filter.
    fn refilter(&mut self) {
        self.visible.clear();
        self.filtered_to = None;
        self.filter_new();
    }

    /// Filters the lines read since last time, and lets go of those the log
    /// has dropped.
    fn filter_new(&mut self) {
        let Some(tail) = self.tails[self.source.index()].as_ref() else {
            self.visible.clear();
            self.counts = Counts::default();
            return;
        };
        let lines = tail.lines();
        let first = lines.first().map_or(0, |line| line.seq);
        self.visible.retain(|seq| *seq >= first);
        let from = match self.filtered_to {
            Some(last) if last >= first => (last + 1 - first) as usize,
            _ => 0,
        };
        let severity = self.severity;
        let query = self.query.as_str();
        self.visible.extend(
            lines[from.min(lines.len())..]
                .iter()
                .filter(|line| severity.admits(line) && matches(line, query))
                .map(|line| line.seq),
        );
        self.filtered_to = lines.last().map(|line| line.seq);
        self.counts = Counts::of(lines);
        if self
            .selected
            .is_some_and(|seq| seq < first || lines.last().is_none_or(|last| seq > last.seq))
        {
            self.selected = None;
        }
    }

    /// The line with `seq` in the log on show.
    fn line(&self, seq: u64) -> Option<&LogLine> {
        let lines = self.tail()?.lines();
        let first = lines.first()?.seq;
        lines.get(seq.checked_sub(first)? as usize)
    }

    fn show_source(&mut self, source: Source, cx: &mut Context<Self>) {
        if self.source == source {
            return;
        }
        self.source = source;
        self.selected = None;
        // Read the log afresh, as it isn't followed while hidden.
        self.tails[source.index()] = None;
        self.refilter();
        self.read(cx);
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn show_severity(&mut self, severity: Severity, cx: &mut Context<Self>) {
        if self.severity != severity {
            self.severity = severity;
            self.refilter();
            self.scroll.scroll_to_bottom();
            cx.notify();
        }
    }

    fn search_changed(&mut self, cx: &mut Context<Self>) {
        let query = self.search.read(cx).value().trim().to_lowercase();
        if query != self.query {
            self.query = query;
            self.refilter();
            self.scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    fn select(&mut self, seq: u64, cx: &mut Context<Self>) {
        self.selected = if self.selected == Some(seq) {
            None
        } else {
            Some(seq)
        };
        cx.notify();
    }

    /// The lines that pass the filters, as written.
    fn visible_text(&self) -> String {
        self.visible
            .iter()
            .filter_map(|seq| self.line(*seq))
            .map(|line| line.raw.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The whole record the selected line belongs to, as written.
    fn selected_record(&self) -> Option<String> {
        let seq = self.selected?;
        let lines = self.tail()?.lines();
        let index = (seq.checked_sub(lines.first()?.seq)?) as usize;
        if index >= lines.len() {
            return None;
        }
        let record = &lines[record_around(lines, index)];
        Some(
            record
                .iter()
                .map(|line| line.raw.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    fn set_detailed(&mut self, on: bool, cx: &mut Context<Self>) {
        logs::set_detailed(on);
        // Tracking only takes it up as it starts.
        if *self.daemon.read(cx).connection() == Connection::Online {
            self.launcher
                .update(cx, |launcher, cx| launcher.restart(cx));
        }
        cx.notify();
    }

    fn save_report(&mut self, cx: &mut Context<Self>) {
        if self.reporting {
            return;
        }
        let daemon = self.daemon.read(cx);
        let status = daemon.status();
        let facts = ReportFacts {
            app_version: format!("{} {}", crate::updates::app_name(), crate::updates::VERSION),
            daemon_version: status
                .and_then(|status| status.daemon.as_ref())
                .map(|daemon| daemon.version.clone()),
            connection: format!("{:?}", daemon.connection()),
            launcher: format!("{:?}", self.launcher.read(cx).activity()),
            status: status.and_then(|status| serde_json::to_string_pretty(status).ok()),
        };
        self.reporting = true;
        self.report = None;
        cx.notify();
        self._report = Some(cx.spawn(async move |this, cx| {
            let saved = cx
                .background_executor()
                .spawn(async move { logs::save_report(&facts) })
                .await;
            this.update(cx, |page, cx| {
                page.reporting = false;
                page.report = Some(match saved {
                    Ok(path) => {
                        cx.reveal_path(&path);
                        Notice::new(Tone::Good, t!("logs.report_saved", path = path.display()))
                    }
                    Err(error) => Notice::error(&t!("logs.report_failed"), &error),
                });
                cx.notify();
            })
            .ok();
        }));
    }

    /// A row of choices, one of them chosen, like Settings' output.
    fn segments<T: Copy + PartialEq + 'static>(
        &self,
        id: &'static str,
        choices: &[(T, SharedString)],
        chosen: T,
        choose: fn(&mut Self, T, &mut Context<Self>),
        cx: &Context<Self>,
    ) -> Div {
        h_flex()
            .flex_none()
            .gap(px(2.))
            .p(px(3.))
            .rounded(px(9.))
            .bg(palette::sunken())
            .border_1()
            .border_color(palette::line())
            .children(choices.iter().enumerate().map(|(index, (choice, label))| {
                let choice = *choice;
                let on = choice == chosen;
                Button::new((id, index))
                    .ghost()
                    .small()
                    .h_7()
                    .px_3()
                    .rounded(px(6.))
                    .label(label.clone())
                    .selected(on)
                    .map(|segment| {
                        if on {
                            segment
                                .bg(Hsla::from(gpui_kit::rgb(0x26262b)))
                                .text_color(palette::text())
                        } else {
                            segment.text_color(palette::text_2())
                        }
                    })
                    .on_click(cx.listener(move |page, _, _, cx| choose(page, choice, cx)))
            }))
    }

    fn toolbar(&self, cx: &Context<Self>) -> Div {
        let sources: Vec<(Source, SharedString)> = Source::ALL
            .into_iter()
            .map(|source| (source, source.label()))
            .collect();
        let counts = self.counts;
        let severities: Vec<(Severity, SharedString)> = Severity::ALL
            .into_iter()
            .map(|severity| {
                let label = match severity {
                    Severity::All => t!("logs.all"),
                    Severity::Warnings => t!("logs.warnings", count = counts.warnings),
                    Severity::Errors => t!("logs.errors", count = counts.errors),
                };
                (severity, label.into())
            })
            .collect();
        let path = self.tail().map(|tail| tail.path().to_path_buf());
        let exists = self.tail().is_some_and(LogTail::exists);
        h_flex()
            .flex_wrap()
            .gap_2()
            .items_center()
            .child(self.segments("log-source", &sources, self.source, Self::show_source, cx))
            .child(self.segments(
                "log-severity",
                &severities,
                self.severity,
                Self::show_severity,
                cx,
            ))
            .child(
                div().flex_1().min_w(px(160.)).child(
                    Input::new(&self.search)
                        .small()
                        .cleanable(true)
                        .prefix(
                            div()
                                .text_color(palette::text_3())
                                .child(Icon::new(IconName::Search).size(px(14.))),
                        )
                        .bg(palette::sunken()),
                ),
            )
            .child(
                icon_button("copy-log", IconName::Copy, t!("logs.copy_shown"))
                    .disabled(self.visible.is_empty())
                    .on_click(cx.listener(|page, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(page.visible_text()))
                    })),
            )
            .child(
                icon_button("show-log-file", IconName::FolderOpen, t!("logs.show_file"))
                    .disabled(!exists)
                    .on_click(move |_, _, cx| {
                        if let Some(path) = &path {
                            cx.reveal_path(path);
                        }
                    }),
            )
    }

    fn row(&self, seq: u64, show_target: bool, cx: &Context<Self>) -> AnyElement {
        let Some(line) = self.line(seq) else {
            return div().h(px(ROW_HEIGHT)).into_any_element();
        };
        let selected = self.selected == Some(seq);
        let (level_color, message_color) = level_colors(line.level);
        let time = line
            .time
            .as_deref()
            .and_then(|time| logs::shift_clock(time, self.offset_minutes))
            .unwrap_or_default();
        let level = match (line.continuation, line.level) {
            (false, Some(level)) => level.as_str(),
            _ => "",
        };
        h_flex()
            .id(("log-line", seq as usize))
            .h(px(ROW_HEIGHT))
            .w_full()
            .px_3()
            .gap_3()
            .cursor_pointer()
            .font_family(MONO_FONT)
            .text_size(px(11.5))
            .when(selected, |row| row.bg(palette::raised()))
            .when(!selected, |row| {
                row.hover(|style| style.bg(palette::inset()))
            })
            .on_click(cx.listener(move |page, _, _, cx| page.select(seq, cx)))
            .child(
                div()
                    .w(px(58.))
                    .flex_none()
                    .text_color(palette::text_3())
                    .child(time),
            )
            .child(
                div()
                    .w(px(40.))
                    .flex_none()
                    .text_color(level_color)
                    .when(line.level == Some(Level::Error), |cell| {
                        cell.font_semibold()
                    })
                    .child(level),
            )
            .when(show_target, |row| {
                row.child(
                    div()
                        .w(px(150.))
                        .flex_none()
                        .truncate()
                        .text_color(palette::text_3())
                        .child(line.target.clone()),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(message_color)
                    .child(line.message.clone()),
            )
            .into_any_element()
    }

    fn list(&self, height: f32, show_target: bool, cx: &Context<Self>) -> AnyElement {
        let tail = self.tail();
        let empty = if !tail.is_some_and(LogTail::exists) {
            Some(self.source.missing())
        } else if self.visible.is_empty() {
            Some(if tail.is_some_and(|tail| tail.lines().is_empty()) {
                t!("logs.empty").into()
            } else {
                t!("logs.no_match").into()
            })
        } else {
            None
        };
        let frame = div()
            .h(px(height))
            .rounded(px(10.))
            .border_1()
            .border_color(palette::line())
            .bg(palette::sunken())
            .overflow_hidden();
        if let Some(empty) = empty {
            return frame
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(palette::text_3())
                .child(empty)
                .into_any_element();
        }
        frame
            .py_1()
            .child(
                uniform_list(
                    "log-lines",
                    self.visible.len(),
                    cx.processor(move |page, range: Range<usize>, _, cx| {
                        range
                            .filter_map(|index| page.visible.get(index).copied())
                            .map(|seq| page.row(seq, show_target, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.scroll)
                .size_full(),
            )
            .into_any_element()
    }

    fn footer(&self, cx: &Context<Self>) -> Div {
        let tail = self.tail();
        let total = tail.map_or(0, |tail| tail.lines().len());
        let mut words = if self.visible.len() == total {
            t!("logs.lines", count = total).to_string()
        } else {
            t!("logs.lines_of", shown = self.visible.len(), count = total).to_string()
        };
        if tail.is_some_and(LogTail::trimmed) {
            words = t!("logs.older_left_out", lines = words).to_string();
        }
        // A daemon started from a console logs there, not to the file.
        let elsewhere = self.source == Source::Tracking
            && *self.daemon.read(cx).connection() == Connection::Online
            && !self.launcher.read(cx).started_daemon();
        let behind = self.scroll.is_scrolled_to_end() == Some(false);
        h_flex()
            .min_h(px(28.))
            .gap_3()
            .justify_between()
            .child(
                div()
                    .min_w_0()
                    .text_xs()
                    .text_color(palette::text_3())
                    .child(if elsewhere {
                        t!("logs.started_elsewhere").to_string()
                    } else {
                        words
                    }),
            )
            .when(behind, |row| {
                row.child(
                    Button::new("log-latest")
                        .ghost()
                        .xsmall()
                        .icon(IconName::ArrowDown)
                        .label(t!("logs.latest"))
                        .on_click(cx.listener(|page, _, _, cx| {
                            page.scroll.scroll_to_bottom();
                            cx.notify();
                        })),
                )
            })
    }

    /// The selected line's whole record, to read and copy.
    fn detail(&self, cx: &Context<Self>) -> Option<Div> {
        let record = self.selected_record()?;
        let copy = record.clone();
        Some(
            v_flex()
                .gap_2()
                .p_3()
                .rounded(px(10.))
                .border_1()
                .border_color(palette::line_strong())
                .bg(palette::inset())
                .child(
                    h_flex()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(palette::text_3())
                                .child(t!("logs.selected_line")),
                        )
                        .child(
                            h_flex()
                                .gap_1()
                                .child(
                                    Button::new("copy-log-line")
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Copy)
                                        .label(t!("logs.copy"))
                                        .on_click(move |_, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                copy.clone(),
                                            ))
                                        }),
                                )
                                .child(
                                    Button::new("close-log-line")
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Close)
                                        .tooltip(t!("logs.close"))
                                        .on_click(cx.listener(|page, _, _, cx| {
                                            page.selected = None;
                                            cx.notify();
                                        })),
                                ),
                        ),
                )
                .child(
                    div()
                        .id("log-line-detail")
                        .max_h(px(220.))
                        .overflow_y_scroll()
                        .font_family(MONO_FONT)
                        .text_size(px(11.5))
                        .line_height(px(17.))
                        .text_color(palette::text())
                        .children(
                            record
                                .lines()
                                .map(|line| div().child(line.to_string()))
                                .collect::<Vec<_>>(),
                        ),
                ),
        )
    }

    fn detailed_card(&self, cx: &Context<Self>) -> Div {
        let title: SharedString = t!("logs.detailed").into();
        card(cx).p(px(18.)).child(
            h_flex()
                .gap_3()
                .items_center()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_0p5()
                        .child(
                            div()
                                .text_sm()
                                .font_medium()
                                .text_color(palette::text())
                                .child(title.clone()),
                        )
                        .child(hint(t!("logs.detailed_hint"), cx)),
                )
                .child(
                    Switch::new("detailed-logging")
                        .checked(logs::detailed())
                        .disabled(self.launcher.read(cx).is_busy())
                        .accessibility_label(title)
                        .on_change(
                            cx.listener(|page, on: &bool, _, cx| page.set_detailed(*on, cx)),
                        ),
                ),
        )
    }
}

fn icon_button(id: &'static str, icon: IconName, tooltip: impl Into<SharedString>) -> Button {
    let tooltip = tooltip.into();
    Button::new(id)
        .ghost()
        .small()
        .size_7()
        .icon(icon)
        .tooltip(tooltip.clone())
        .accessibility_label(tooltip)
}

impl Render for LogsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let height = (window.viewport_size().height.as_f32() - AROUND_LIST).max(MIN_LIST_HEIGHT);
        let show_target = vrft_gui_core::content_width(window) >= TARGET_MIN_WIDTH;
        v_flex()
            .gap_6()
            .child(
                PageHeader::new(t!("logs.title"))
                    .description(t!("logs.description"))
                    .trailing(
                        Button::new("save-report")
                            .primary()
                            .regular()
                            .icon(IconName::FileText)
                            .label(t!("logs.save_report"))
                            .loading(self.reporting)
                            .disabled(self.reporting)
                            .tooltip(t!("logs.save_report_hint"))
                            .on_click(cx.listener(|page, _, _, cx| page.save_report(cx))),
                    ),
            )
            .children(self.report.clone())
            .child(
                card(cx)
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(self.toolbar(cx))
                    .child(self.list(height, show_target, cx))
                    .child(self.footer(cx))
                    .children(self.detail(cx)),
            )
            .child(self.detailed_card(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(raw: &[&str]) -> Vec<LogLine> {
        let mut lines: Vec<LogLine> = Vec::new();
        for (seq, raw) in raw.iter().enumerate() {
            let line = LogLine::parse(raw, seq as u64, lines.last());
            lines.push(line);
        }
        lines
    }

    #[test]
    fn a_record_takes_its_following_lines() {
        let lines = lines(&[
            "[2026-09-25T14:47:47Z INFO  vrft_d] one",
            "[2026-09-25T14:47:47Z ERROR vrft_d] two:",
            "  cause a",
            "  cause b",
            "[2026-09-25T14:47:48Z INFO  vrft_d] three",
        ]);
        assert_eq!(record_around(&lines, 0), 0..1);
        assert_eq!(record_around(&lines, 1), 1..4);
        assert_eq!(record_around(&lines, 3), 1..4);
        assert_eq!(record_around(&lines, 4), 4..5);
    }

    #[test]
    fn counts_records_not_their_lines() {
        let lines = lines(&[
            "[2026-09-25T14:47:47Z WARN  vrft_d] careful",
            "[2026-09-25T14:47:47Z ERROR vrft_d] broke:",
            "  because",
            "[2026-09-25T14:47:48Z ERROR vrft_d] again",
        ]);
        assert_eq!(
            Counts::of(&lines),
            Counts {
                errors: 2,
                warnings: 1
            }
        );
        assert!(Severity::Errors.admits(&lines[2]));
        assert!(!Severity::Errors.admits(&lines[0]));
        assert!(matches(&lines[1], "broke"));
        assert!(matches(&lines[1], ""));
        assert!(!matches(&lines[1], "fine"));
    }
}
