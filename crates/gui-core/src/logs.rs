//! The logs: where the daemon's and this app's go, reading one as it grows,
//! detailed logging, and the report that gathers them up for someone helping.
//!
//! Both programs log through `env_logger`, so every line starts
//! `[2026-09-25T14:47:47Z INFO  vrft_d] `. Each keeps the log of its run
//! before as `<name>.previous.log`, so starting again doesn't lose what
//! happened to the last one.
use log::Level;
use std::fs::{self, File};
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// The daemon's output, when this app started it.
pub const DAEMON_LOG: &str = "vrft_d.log";
/// This app's own log.
pub const APP_LOG: &str = "vrft_app.log";
/// Where saved reports go, in the data folder.
pub const REPORTS_DIR: &str = "reports";

/// What both programs log while detailed logging is on: everything of
/// VRFT's and its modules', and only what the chattiest libraries say at
/// their normal level.
pub const DETAILED_FILTER: &str = "debug,mdns_sd=info,wgpu=warn,naga=warn,cubecl=info,burn=info,\
     ureq=info,rustls=info,hyper=info,axum=info,tower=info,mio=info,h2=info,tokio=info,\
     gpui=info,blade=info,zbus=info,cosmic_text=info,velopack=info";

/// The most lines a [`LogTail`] keeps; older ones are dropped.
const MAX_LINES: usize = 20_000;
/// How much of a long log is read when it's first opened: its end.
const FIRST_READ: u64 = 2 * 1024 * 1024;
/// How much of a log's start identifies it, to notice it was replaced.
const HEAD: usize = 64;

/// The daemon's log, in the folder it runs in.
pub fn daemon_log() -> Option<PathBuf> {
    Some(crate::paths::data_dir()?.join(DAEMON_LOG))
}

/// This app's log.
pub fn app_log() -> Option<PathBuf> {
    Some(crate::paths::data_dir()?.join(APP_LOG))
}

/// The log `path` was in its program's last run: `vrft_d.log` is followed by
/// `vrft_d.previous.log`.
pub fn previous(path: &Path) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!("{stem}.previous.log"))
}

/// Keeps the log at `path` as the previous run's, before a new run starts
/// one there.
pub fn keep_previous(path: &Path) {
    if !path.is_file() {
        return;
    }
    let previous = previous(path);
    // Renaming over a file isn't allowed everywhere.
    let _ = fs::remove_file(&previous);
    if let Err(error) = fs::rename(path, &previous) {
        log::debug!(
            "Couldn't keep {} as the previous log: {error}",
            path.display()
        );
    }
}

static DETAILED: AtomicBool = AtomicBool::new(false);

/// Whether both programs log in detail, until this app closes.
pub fn detailed() -> bool {
    DETAILED.load(Ordering::Relaxed)
}

/// Turns detailed logging on or off: at once for this app, and for the
/// daemon from when it next starts. A `RUST_LOG` set by hand keeps the app
/// at that.
pub fn set_detailed(on: bool) {
    DETAILED.store(on, Ordering::Relaxed);
    if std::env::var_os("RUST_LOG").is_none() {
        log::set_max_level(if on {
            log::LevelFilter::Debug
        } else {
            log::LevelFilter::Info
        });
    }
    log::info!("Detailed logging is {}", if on { "on" } else { "off" });
}

/// What the daemon about to start logs: [`DETAILED_FILTER`] while detailed
/// logging is on, otherwise whatever `RUST_LOG` says, or the daemon's own
/// default.
pub fn daemon_filter() -> Option<&'static str> {
    detailed().then_some(DETAILED_FILTER)
}

/// Logs a problem the app shows, unless it's the same as the last one, so
/// one shown on every poll is logged once.
pub fn record_problem(text: &str) {
    static LAST: Mutex<String> = Mutex::new(String::new());
    let Ok(mut last) = LAST.lock() else {
        return;
    };
    if *last != text {
        log::warn!("{text}");
        *last = text.to_string();
    }
}

/// One line of a log.
#[derive(Debug, Clone, PartialEq)]
pub struct LogLine {
    /// Counts up through everything a [`LogTail`] has read, so a line can be
    /// told apart from those read before or after it.
    pub seq: u64,
    /// The record's level. A line that carries on the record before it, such
    /// as a backtrace's, has that record's.
    pub level: Option<Level>,
    /// When it was logged, as UTC `HH:MM:SS`, when the line says.
    pub time: Option<String>,
    /// Where it was logged from, such as `vrft_d` or a module's name.
    pub target: String,
    pub message: String,
    /// It carries on the record before it.
    pub continuation: bool,
    /// The line as written.
    pub raw: String,
}

impl LogLine {
    /// Reads `raw`, a line written by `env_logger`. A line it didn't write,
    /// such as a panic's, carries on `before`.
    pub fn parse(raw: &str, seq: u64, before: Option<&LogLine>) -> Self {
        if let Some((level, time, target, message)) = parse_header(raw) {
            return Self {
                seq,
                level: Some(level),
                time,
                target: target.to_string(),
                message: message.to_string(),
                continuation: false,
                raw: raw.to_string(),
            };
        }
        Self {
            seq,
            level: before.and_then(|line| line.level),
            time: None,
            target: String::new(),
            message: raw.to_string(),
            continuation: true,
            raw: raw.to_string(),
        }
    }

    /// Whether it's at least as severe as `level`, such as a warning or an
    /// error for [`Level::Warn`]. A line of unknown level shows at every
    /// level, as it may be a crash's.
    pub fn at_least(&self, level: Level) -> bool {
        self.level.is_none_or(|own| own <= level)
    }
}

/// The level, time, target and message of a line `env_logger` wrote:
/// `[2026-09-25T14:47:47Z INFO  vrft_d] Starting...`.
fn parse_header(raw: &str) -> Option<(Level, Option<String>, &str, &str)> {
    let rest = raw.strip_prefix('[')?;
    let (header, message) = rest.split_once("] ").or_else(|| {
        // A record with an empty message.
        rest.strip_suffix(']').map(|header| (header, ""))
    })?;
    let mut parts = header.split_whitespace();
    let first = parts.next()?;
    let (time, level) = match first.parse::<Level>() {
        // No timestamp, as `env_logger` writes when told not to.
        Ok(level) => (None, level),
        Err(_) => (Some(first), parts.next()?.parse::<Level>().ok()?),
    };
    let target = parts.next().unwrap_or("");
    let time = time.and_then(|time| {
        let clock = time.split_once('T')?.1.get(..8)?;
        let digits = clock.bytes().filter(u8::is_ascii_digit).count();
        (digits == 6).then(|| clock.to_string())
    });
    Some((level, time, target, message))
}

/// What reading a log again found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TailChange {
    None,
    /// Lines were added at the end.
    Grew,
    /// It's a different log now, such as one a restart began.
    Replaced,
}

/// A log read as it grows: its last [`MAX_LINES`] lines, and whatever is
/// added to it each time it's [`polled`](Self::poll).
pub struct LogTail {
    path: PathBuf,
    /// How far into the file has been read.
    offset: u64,
    /// The file's first bytes, to notice another file in its place.
    head: Vec<u8>,
    /// What's been read after the last full line.
    partial: Vec<u8>,
    lines: Vec<LogLine>,
    next_seq: u64,
    /// Only its end was read, as it was long.
    trimmed: bool,
    exists: bool,
}

impl LogTail {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            offset: 0,
            head: Vec::new(),
            partial: Vec::new(),
            lines: Vec::new(),
            next_seq: 0,
            trimmed: false,
            exists: false,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn lines(&self) -> &[LogLine] {
        &self.lines
    }

    /// Whether the file was there when last read.
    pub fn exists(&self) -> bool {
        self.exists
    }

    /// Whether lines from its start were left out, as it was long.
    pub fn trimmed(&self) -> bool {
        self.trimmed
    }

    /// Reads what's been added since last time.
    pub fn poll(&mut self) -> TailChange {
        let Ok(mut file) = File::open(&self.path) else {
            let had = self.exists || !self.lines.is_empty();
            self.reset();
            return if had {
                TailChange::Replaced
            } else {
                TailChange::None
            };
        };
        let length = file.metadata().map(|meta| meta.len()).unwrap_or(0);
        let mut head = vec![0; HEAD.min(length as usize)];
        if file.read_exact(&mut head).is_err() {
            return TailChange::None;
        }
        let shared = head.len().min(self.head.len());
        let replaced = length < self.offset || head[..shared] != self.head[..shared];
        let mut change = TailChange::None;
        if replaced {
            self.reset();
            change = TailChange::Replaced;
        }
        self.exists = true;
        if head.len() > self.head.len() {
            self.head = head;
        }
        if self.offset == 0 && length > FIRST_READ {
            self.offset = length - FIRST_READ;
            self.trimmed = true;
        }
        if length <= self.offset {
            return change;
        }
        let mut added = Vec::with_capacity((length - self.offset) as usize);
        let read = file
            .seek(SeekFrom::Start(self.offset))
            .and_then(|_| file.take(length - self.offset).read_to_end(&mut added));
        let Ok(read) = read else {
            return change;
        };
        let skip_partial_first = self.trimmed && self.offset > 0 && self.lines.is_empty();
        self.offset += read as u64;
        self.partial.extend_from_slice(&added);
        let mut text = std::mem::take(&mut self.partial);
        let complete = match text.iter().rposition(|&byte| byte == b'\n') {
            Some(end) => {
                self.partial = text.split_off(end + 1);
                text
            }
            None => {
                self.partial = text;
                return change;
            }
        };
        let mut rows = complete.split(|&byte| byte == b'\n');
        if skip_partial_first {
            // Reading from partway through, the first line is cut off.
            rows.next();
        }
        let before = self.lines.len();
        for row in rows {
            let raw = String::from_utf8_lossy(row);
            let raw = raw.trim_end_matches('\r');
            if raw.is_empty() {
                continue;
            }
            let line = LogLine::parse(raw, self.next_seq, self.lines.last());
            self.next_seq += 1;
            self.lines.push(line);
        }
        if self.lines.len() > MAX_LINES {
            let excess = self.lines.len() - MAX_LINES;
            self.lines.drain(..excess);
            self.trimmed = true;
        }
        if change == TailChange::None && self.lines.len() != before {
            change = TailChange::Grew;
        }
        change
    }

    fn reset(&mut self) {
        self.offset = 0;
        self.head.clear();
        self.partial.clear();
        self.lines.clear();
        self.trimmed = false;
        self.exists = false;
    }
}

/// The last `count` lines of the file at `path`, read from no further back
/// than its last `limit` bytes.
pub fn last_lines(path: &Path, count: usize, limit: u64) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(limit);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text.lines().collect();
    if start > 0 && !lines.is_empty() {
        // Cut off partway through.
        lines.remove(0);
    }
    let from = lines.len().saturating_sub(count);
    Some(lines[from..].join("\n"))
}

/// The UTC date and time `seconds` after the Unix epoch, as
/// `(year, month, day, hour, minute, second)`.
pub fn utc_parts(seconds: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (seconds / 86_400) as i64;
    let in_day = seconds % 86_400;
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (
        year,
        month,
        day,
        (in_day / 3600) as u32,
        (in_day / 60 % 60) as u32,
        (in_day % 60) as u32,
    )
}

/// Minutes to add to UTC for this PC's local time, as it is now.
#[cfg(windows)]
pub fn local_offset_minutes() -> i32 {
    use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    const TIME_ZONE_ID_STANDARD: u32 = 1;
    const TIME_ZONE_ID_DAYLIGHT: u32 = 2;
    let mut zone = TIME_ZONE_INFORMATION::default();
    // SAFETY: `zone` is written in place.
    let id = unsafe { GetTimeZoneInformation(&mut zone) };
    let bias = zone.Bias
        + match id {
            TIME_ZONE_ID_STANDARD => zone.StandardBias,
            TIME_ZONE_ID_DAYLIGHT => zone.DaylightBias,
            _ => 0,
        };
    -bias
}

#[cfg(not(windows))]
pub fn local_offset_minutes() -> i32 {
    0
}

/// `time`, UTC `HH:MM:SS`, moved by `offset_minutes`, such as to local time.
pub fn shift_clock(time: &str, offset_minutes: i32) -> Option<String> {
    let mut parts = time.split(':').map(|part| part.parse::<i64>().ok());
    let (hours, minutes, seconds) = (parts.next()??, parts.next()??, parts.next()??);
    let total =
        (hours * 3600 + minutes * 60 + seconds + i64::from(offset_minutes) * 60).rem_euclid(86_400);
    Some(format!(
        "{:02}:{:02}:{:02}",
        total / 3600,
        total / 60 % 60,
        total % 60
    ))
}

/// What a report says about the PC and the app, besides the logs.
pub struct ReportFacts {
    pub app_version: String,
    pub daemon_version: Option<String>,
    /// How the app reaches the daemon, such as `Online`.
    pub connection: String,
    /// What the app is doing to the daemon, such as starting it.
    pub launcher: String,
    /// The daemon's latest status, as JSON.
    pub status: Option<String>,
}

/// Gathers `facts`, `config.json` and the end of each log into one text
/// file in the data folder's reports, and says where it is.
pub fn save_report(facts: &ReportFacts) -> anyhow::Result<PathBuf> {
    use anyhow::Context as _;
    let data = crate::paths::data_dir().context("Can't find this app's folder")?;
    let folder = data.join(REPORTS_DIR);
    fs::create_dir_all(&folder).with_context(|| format!("Couldn't make {}", folder.display()))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    let (year, month, day, hour, minute, second) = utc_parts(now);
    let path = folder.join(format!(
        "vrft-report-{year}{month:02}{day:02}-{hour:02}{minute:02}{second:02}.txt"
    ));
    let mut sections = vec![report_header(facts, now)];
    if let Some(status) = &facts.status {
        sections.push(section("Status", status));
    }
    if let Some(config) = crate::paths::config_file() {
        let text = fs::read_to_string(&config)
            .unwrap_or_else(|error| format!("Couldn't read it: {error}"));
        sections.push(section(&config.display().to_string(), &text));
    }
    let logs = [daemon_log(), app_log()];
    for log in logs.into_iter().flatten() {
        for (path, count) in [(log.clone(), 3000), (previous(&log), 1000)] {
            if let Some(text) = last_lines(&path, count, 4 * 1024 * 1024) {
                let title = format!("{} (last {count} lines)", path.display());
                sections.push(section(&title, &text));
            }
        }
    }
    fs::write(&path, sections.join("\n"))
        .with_context(|| format!("Couldn't write {}", path.display()))?;
    log::info!("Saved a report to {}", path.display());
    Ok(path)
}

fn report_header(facts: &ReportFacts, now: u64) -> String {
    let (year, month, day, hour, minute, second) = utc_parts(now);
    let mut text = format!(
        "VRFaceTracking report\n\
         Made: {year}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} UTC\n\
         App: {}\n",
        facts.app_version
    );
    if let Some(version) = &facts.daemon_version {
        text += &format!("Tracking: {version}\n");
    }
    text += &format!(
        "System: {} {}\nConnection: {}\nApp is: {}\nDetailed logging: {}\n",
        std::env::consts::OS,
        std::env::consts::ARCH,
        facts.connection,
        facts.launcher,
        if detailed() { "on" } else { "off" },
    );
    text
}

fn section(title: &str, body: &str) -> String {
    format!("\n===== {title} =====\n{}\n", body.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn reads_env_logger_lines() {
        let line = LogLine::parse(
            "[2026-09-25T14:47:47Z ERROR vrft_d::modules] Couldn't load: no such file",
            7,
            None,
        );
        assert_eq!(line.level, Some(Level::Error));
        assert_eq!(line.time.as_deref(), Some("14:47:47"));
        assert_eq!(line.target, "vrft_d::modules");
        assert_eq!(line.message, "Couldn't load: no such file");
        assert!(!line.continuation);
        assert_eq!(line.seq, 7);

        let padded = LogLine::parse("[2026-09-25T14:47:47Z INFO  vrft_d] Starting...", 0, None);
        assert_eq!(padded.level, Some(Level::Info));
        assert_eq!(padded.message, "Starting...");
    }

    #[test]
    fn other_lines_carry_on_the_record_before() {
        let error = LogLine::parse("[2026-09-25T14:47:47Z ERROR vrft_d] It broke:", 0, None);
        let more = LogLine::parse("   0: std::backtrace", 1, Some(&error));
        assert!(more.continuation);
        assert_eq!(more.level, Some(Level::Error));
        assert_eq!(more.message, "   0: std::backtrace");
        let alone = LogLine::parse("thread 'main' panicked", 0, None);
        assert_eq!(alone.level, None);
        assert!(alone.at_least(Level::Error));
        // Square brackets that aren't a header.
        let bracketed = LogLine::parse("[not a header] text", 0, None);
        assert!(bracketed.continuation);
    }

    #[test]
    fn filters_by_level() {
        let warn = LogLine::parse("[2026-09-25T14:47:47Z WARN  x] w", 0, None);
        let debug = LogLine::parse("[2026-09-25T14:47:47Z DEBUG x] d", 0, None);
        assert!(warn.at_least(Level::Warn));
        assert!(!warn.at_least(Level::Error));
        assert!(!debug.at_least(Level::Info));
        assert!(debug.at_least(Level::Trace));
    }

    #[test]
    fn previous_logs_sit_beside() {
        assert_eq!(
            previous(Path::new("C:/VRFT/data/vrft_d.log")),
            Path::new("C:/VRFT/data/vrft_d.previous.log")
        );
    }

    fn temp_log(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vrft_logs_{tag}_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir.join("vrft_d.log")
    }

    #[test]
    fn follows_a_growing_log_and_notices_a_new_one() {
        let path = temp_log("tail");
        let mut tail = LogTail::new(path.clone());
        assert_eq!(tail.poll(), TailChange::None);
        assert!(!tail.exists());

        let mut file = File::create(&path).unwrap();
        write!(
            file,
            "[2026-09-25T14:47:47Z INFO  vrft_d] one\n[2026-09-25T14:47:48Z INFO  vrft_d] tw"
        )
        .unwrap();
        assert_eq!(tail.poll(), TailChange::Grew);
        assert_eq!(tail.lines().len(), 1);
        // The rest of a line arrives later.
        writeln!(file, "o").unwrap();
        assert_eq!(tail.poll(), TailChange::Grew);
        assert_eq!(tail.lines()[1].message, "two");
        assert_eq!(tail.poll(), TailChange::None);
        drop(file);

        // A restart keeps that log as the previous one and begins another.
        keep_previous(&path);
        assert!(previous(&path).is_file());
        fs::write(&path, "[2026-09-25T15:00:00Z WARN  vrft_d] fresh\n").unwrap();
        assert_eq!(tail.poll(), TailChange::Replaced);
        assert_eq!(tail.lines().len(), 1);
        assert_eq!(tail.lines()[0].message, "fresh");
        assert!(tail.lines()[0].seq > 1);

        fs::remove_file(&path).unwrap();
        assert_eq!(tail.poll(), TailChange::Replaced);
        assert!(tail.lines().is_empty());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn reads_only_the_end_of_a_long_log() {
        let path = temp_log("long");
        let line = format!("[2026-09-25T14:47:47Z INFO  vrft_d] {}\n", "x".repeat(200));
        let count = (FIRST_READ as usize / line.len()) * 2;
        fs::write(&path, line.repeat(count)).unwrap();
        let mut tail = LogTail::new(path.clone());
        tail.poll();
        assert!(tail.trimmed());
        assert!(tail.lines().len() < count);
        // Every line read is whole.
        assert!(tail.lines().iter().all(|line| !line.continuation));

        let end = last_lines(&path, 3, 10_000).unwrap();
        assert_eq!(end.lines().count(), 3);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn dates_from_unix_seconds() {
        assert_eq!(utc_parts(0), (1970, 1, 1, 0, 0, 0));
        // 2026-09-25T14:47:47Z
        assert_eq!(utc_parts(1_790_347_667), (2026, 9, 25, 14, 47, 47));
        // A leap day.
        assert_eq!(utc_parts(951_782_400), (2000, 2, 29, 0, 0, 0));
    }

    #[test]
    fn shifts_a_clock_across_midnight() {
        assert_eq!(shift_clock("14:47:47", 60).as_deref(), Some("15:47:47"));
        assert_eq!(shift_clock("00:30:00", -60).as_deref(), Some("23:30:00"));
        assert_eq!(shift_clock("23:30:00", 330).as_deref(), Some("05:00:00"));
        assert_eq!(shift_clock("nonsense", 0), None);
    }
}
