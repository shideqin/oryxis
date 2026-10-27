//! Event-loop stall watchdog behind the debug-logging toggle (#104).
//!
//! The freeze class in issue #104 does not reproduce anywhere but the
//! reporter's machine, and the one recording we have shows the app not
//! drawing at all: not a slow frame, not a slow message. What decides
//! that issue (and any future "the UI froze" report) is knowing WHICH
//! of four layers stopped, and this module is the instrument:
//!
//! * update heartbeat dead + a message in flight  -> that handler is
//!   blocked; the log names it.
//! * update heartbeat dead + nothing in flight    -> the winit event
//!   loop itself stopped dispatching (platform / driver territory).
//! * update alive + view heartbeat dead while real messages flow ->
//!   iced stopped drawing (presentation / wgpu / compositor).
//! * everything alive but the user sees a freeze  -> pixels leave the
//!   app and die later; the compositor is the suspect.
//!
//! Issue #273 added the two things those four layers could not say:
//!
//! * WHERE a silent loop is stuck: inside `view()`, inside the terminal
//!   widget's draw or event handling (`oryxis_terminal::diag`), or
//!   outside our code entirely; and, on Linux, whether the UI thread is
//!   spinning in user space or blocked in a system call, read from
//!   `/proc` (a tight loop of ours and a driver waiting on the GPU look
//!   the same from inside every other layer).
//! * a MESSAGE STORM: updates and views per second, with the messages
//!   that dominate them, so a livelock that keeps every layer beating
//!   (a widget republishing on each redraw, a handler feeding itself)
//!   names its driver instead of hiding behind a healthy heartbeat.
//!
//! Everything is gated on [`crate::logging::is_enabled`], the same
//! switch as the debug-log file, so a normal session pays one relaxed
//! atomic load per message and nothing else. While enabled, a 500 ms
//! `Message::NoOp` tick (see `subscription.rs`) guarantees the update
//! heartbeat beats on an idle-but-healthy loop, which is what lets the
//! watchdog tell "idle" apart from "dead".

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// A message handler running longer than this gets its own log line.
const SLOW_MESSAGE_MS: u64 = 250;
/// No update heartbeat for this long counts as a stall.
const STALL_MS: u64 = 2_000;
/// While a stall persists, re-log at most this often.
const STALL_RELOG_MS: u64 = 5_000;
/// View not running for this long, while real messages flow, counts as
/// a presentation stall.
const VIEW_STALL_MS: u64 = 3_000;
/// How many recent message names the stall report carries.
const RECENT_CAP: usize = 24;
/// Budget for one formatted message name; payloads (PTY chunks!) must
/// never bloat the ring.
const NAME_BUDGET: usize = 96;
/// A second with at least this many messages (outermost `update()`
/// passes) counts toward a storm. Measured 2026-10-26 with opencode
/// 1.19.4 streaming its thinking over SSH, the heaviest TUI load we have:
/// about 100 messages and 50 views a second at the peak, the PTY and SSH
/// readers batching what is already queued.
const STORM_UPDATES_PER_S: f64 = 400.0;
/// Same for `view()`. A widget that republishes on every redraw rebuilds
/// the tree up to three times a frame, 180 a second at 60 Hz, while the
/// load above stays near 50.
const STORM_VIEWS_PER_S: f64 = 200.0;
/// One message kind this often is a storm by itself, whatever the totals:
/// a hover or scroll notification republished on each redraw is a few a
/// second when healthy and a frame rate when looping. The byte streams
/// ([`is_byte_stream`]) are exempt, they are high by nature.
const STORM_KIND_PER_S: f64 = 100.0;
/// How long the rate has to hold before it is reported.
const STORM_MIN_SECS: f64 = 3.0;
/// While a storm persists, report it at most this often.
const STORM_RELOG_MS: u64 = 15_000;
/// How many message kinds a storm report names.
const STORM_TOP: usize = 6;
/// Sentinel for "the UI thread is not inside `view()`".
const NOT_IN_VIEW: u64 = u64::MAX;

/// Milliseconds since the first use of the watchdog in this process.
/// One shared clock so the atomics below can carry plain `u64`s.
fn now_ms() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Last time `update()` was entered (any message, ticks included).
static UPDATE_BEAT: AtomicU64 = AtomicU64::new(0);
/// Last time `update()` was entered for a NON-tick message. `NoOp` is
/// the watchdog's own pacemaker (and the latency re-render tick), so it
/// proves the loop is alive but not that the app is doing anything.
static REAL_BEAT: AtomicU64 = AtomicU64::new(0);
/// Last time `view()` was entered.
static VIEW_BEAT: AtomicU64 = AtomicU64::new(0);
/// When the current `view()` pass began, [`NOT_IN_VIEW`] outside one.
static IN_VIEW_SINCE: AtomicU64 = AtomicU64::new(NOT_IN_VIEW);
/// `update()` and `view()` passes since the storm meter last read them.
static UPDATES: AtomicU64 = AtomicU64::new(0);
static VIEWS: AtomicU64 = AtomicU64::new(0);
static THREAD_STARTED: AtomicBool = AtomicBool::new(false);

/// The message currently inside `update()`, if any: (entered_at_ms, name).
static IN_FLIGHT: Mutex<Option<(u64, String)>> = Mutex::new(None);
/// The last [`RECENT_CAP`] completed messages: (entered_at_ms, name, took_ms).
static RECENT: Mutex<VecDeque<(u64, String, u64)>> = Mutex::new(VecDeque::new());
/// Message kinds ([`message_key`]) seen since the storm meter last read them.
static STORM_NAMES: Mutex<Option<HashMap<String, u32>>> = Mutex::new(None);
thread_local! {
    /// How many `update()` passes are open on this thread: the wrapper
    /// messages (`InWindow`, `ForPane`, `ForVault`) dispatch their inner
    /// message through `update()` again, and a storm counts MESSAGES, not
    /// nesting levels.
    static DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// The UI thread's kernel thread id, taken on the first `update()` (which
/// iced always runs there). `None` inside when it could not be read.
#[cfg(target_os = "linux")]
static MAIN_TID: OnceLock<Option<u32>> = OnceLock::new();

/// Diagnostics must never take the app down: shrug off poisoning.
fn lock<T>(m: &'static Mutex<T>) -> MutexGuard<'static, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `{message:?}` cut at [`NAME_BUDGET`] bytes. The `fmt::Write` error
/// aborts the Debug walk early, so a 64 KB PTY chunk costs nothing past
/// the budget.
fn message_name(message: &crate::app::Message) -> String {
    struct Truncate {
        buf: String,
    }
    impl std::fmt::Write for Truncate {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            for ch in s.chars() {
                if self.buf.len() + ch.len_utf8() > NAME_BUDGET {
                    self.buf.push('…');
                    return Err(std::fmt::Error);
                }
                self.buf.push(ch);
            }
            Ok(())
        }
    }
    let mut t = Truncate { buf: String::with_capacity(NAME_BUDGET + 4) };
    let _ = write!(t, "{message:?}");
    t.buf
}

/// A message's KIND for the storm tally: its formatted name with the
/// payload taken out, so `PtyOutput(<pane id>, [27, 91, ...])` from two
/// panes and two batches counts as one kind. Payload is a string
/// literal, anything inside `[...]`, and any token that is a number or
/// an id (hex digits, dashes, dots, with at least one digit). Cut at 64
/// bytes.
fn message_key(name: &str) -> String {
    const CAP: usize = 64;
    fn flush(token: &mut String, out: &mut String) {
        let payload = token.chars().any(|c| c.is_ascii_digit())
            && token
                .chars()
                .all(|c| c.is_ascii_hexdigit() || matches!(c, '-' | '.' | '_'));
        if !payload {
            out.push_str(token);
        }
        token.clear();
    }
    let mut out = String::with_capacity(CAP);
    let mut token = String::new();
    let mut in_string = false;
    let mut escaped = false;
    let mut depth = 0u32;
    for ch in name.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if depth > 0 {
            match ch {
                '[' => depth += 1,
                ']' => depth -= 1,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => {
                flush(&mut token, &mut out);
                in_string = true;
            }
            '[' => {
                flush(&mut token, &mut out);
                depth = 1;
            }
            c if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') => token.push(c),
            // The ellipsis `message_name` appends at its budget.
            '\u{2026}' => flush(&mut token, &mut out),
            c => {
                flush(&mut token, &mut out);
                out.push(c);
            }
        }
        if out.len() >= CAP {
            break;
        }
    }
    flush(&mut token, &mut out);
    if out.len() > CAP {
        let mut cut = CAP;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
    }
    out
}

/// RAII marker for one `update()` pass. Created at the top of
/// `Oryxis::update`, dropped on every exit path (early returns
/// included), which is exactly the window the watchdog attributes to
/// the message.
pub(crate) struct MessageGuard {
    entered: u64,
    armed: bool,
}

/// A wrapper that only hands its inner message to `update()` again: the
/// inner one is what the storm tally names.
fn is_wrapper(message: &crate::app::Message) -> bool {
    matches!(
        message,
        crate::app::Message::InWindow(..)
            | crate::app::Message::ForPane(..)
            | crate::app::Message::ForVault(..)
    )
}

/// Kinds that carry a byte stream and are high by nature: output from a
/// pane, and input the terminal widget writes (keys, mouse reports under
/// a TUI's any-event tracking).
fn is_byte_stream(key: &str) -> bool {
    key.starts_with("Terminal(PtyOutput(") || key.starts_with("Terminal(TerminalInput(")
}

pub(crate) fn message_guard(message: &crate::app::Message) -> MessageGuard {
    if !crate::logging::is_enabled() {
        return MessageGuard { entered: 0, armed: false };
    }
    ensure_started();
    #[cfg(target_os = "linux")]
    MAIN_TID.get_or_init(current_tid);
    let now = now_ms();
    UPDATE_BEAT.store(now, Ordering::Relaxed);
    if !matches!(message, crate::app::Message::NoOp) {
        REAL_BEAT.store(now, Ordering::Relaxed);
    }
    let name = message_name(message);
    let outermost = DEPTH.with(|depth| {
        let open = depth.get();
        depth.set(open + 1);
        open == 0
    });
    if outermost {
        UPDATES.fetch_add(1, Ordering::Relaxed);
    }
    if !is_wrapper(message) {
        *lock(&STORM_NAMES).get_or_insert_with(HashMap::new).entry(message_key(&name)).or_insert(0) += 1;
    }
    *lock(&IN_FLIGHT) = Some((now, name));
    MessageGuard { entered: now, armed: true }
}

impl Drop for MessageGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
        let now = now_ms();
        UPDATE_BEAT.store(now, Ordering::Relaxed);
        if let Some((entered, name)) = lock(&IN_FLIGHT).take() {
            let took = now.saturating_sub(entered);
            if took >= SLOW_MESSAGE_MS {
                tracing::warn!("stall-watchdog: slow message ({took} ms): {name}");
            }
            let mut recent = lock(&RECENT);
            if recent.len() == RECENT_CAP {
                recent.pop_front();
            }
            recent.push_back((entered, name, took));
        }
        let _ = self.entered;
    }
}

/// Heartbeat from the top of `Oryxis::view`, held for the whole pass so
/// a loop stuck INSIDE the view is told from one stuck elsewhere.
pub(crate) struct ViewGuard {
    armed: bool,
}

pub(crate) fn view_guard() -> ViewGuard {
    if !crate::logging::is_enabled() {
        return ViewGuard { armed: false };
    }
    let now = now_ms();
    VIEW_BEAT.store(now, Ordering::Relaxed);
    VIEWS.fetch_add(1, Ordering::Relaxed);
    IN_VIEW_SINCE.store(now, Ordering::Relaxed);
    ViewGuard { armed: true }
}

impl Drop for ViewGuard {
    fn drop(&mut self) {
        if self.armed {
            IN_VIEW_SINCE.store(NOT_IN_VIEW, Ordering::Relaxed);
        }
    }
}

/// Where the UI thread is, for a loop that went silent with no handler
/// in flight: inside our view, inside the terminal widget, or outside
/// anything of ours.
fn silent_loop_location(now: u64) -> String {
    let in_view = IN_VIEW_SINCE.load(Ordering::Relaxed);
    if in_view != NOT_IN_VIEW {
        return format!("inside view() for {} ms", now.saturating_sub(in_view));
    }
    match oryxis_terminal::diag::in_flight() {
        Some((oryxis_terminal::diag::Phase::Draw, held)) => {
            format!("inside the terminal widget's draw for {} ms", held.as_millis())
        }
        Some((oryxis_terminal::diag::Phase::Event, held)) => {
            format!("inside the terminal widget's event handling for {} ms", held.as_millis())
        }
        None => "outside Oryxis code (iced layout / draw / present, the driver or the compositor)"
            .to_string(),
    }
}

/// Tally of `update()` / `view()` rates, read once a second by the
/// monitor. Pure, so the thresholds are testable without a live loop.
#[derive(Default)]
struct StormMeter {
    /// Seconds the rate has held above a threshold, in a row.
    high_secs: f64,
    /// Seconds, updates, views and kinds since the last report of this run.
    window_secs: f64,
    updates: u64,
    views: u64,
    names: HashMap<String, u32>,
    /// When the last report went out (`now_ms`), `None` before the first.
    last_report: Option<u64>,
}

impl StormMeter {
    /// Feed one interval; a report string when a storm is due one.
    fn observe(
        &mut self,
        now: u64,
        secs: f64,
        updates: u64,
        views: u64,
        names: HashMap<String, u32>,
    ) -> Option<String> {
        if secs <= 0.0 {
            return None;
        }
        let busiest_kind = names
            .iter()
            .filter(|(name, _)| !is_byte_stream(name))
            .map(|(_, n)| *n)
            .max()
            .unwrap_or(0);
        let high = updates as f64 / secs >= STORM_UPDATES_PER_S
            || views as f64 / secs >= STORM_VIEWS_PER_S
            || busiest_kind as f64 / secs >= STORM_KIND_PER_S;
        if !high {
            *self = StormMeter { last_report: self.last_report, ..StormMeter::default() };
            return None;
        }
        self.high_secs += secs;
        self.window_secs += secs;
        self.updates += updates;
        self.views += views;
        for (name, n) in names {
            *self.names.entry(name).or_insert(0) += n;
        }
        let due = self.high_secs >= STORM_MIN_SECS
            && self.last_report.is_none_or(|at| now.saturating_sub(at) >= STORM_RELOG_MS);
        if !due {
            return None;
        }
        let mut top: Vec<(String, u32)> = std::mem::take(&mut self.names).into_iter().collect();
        top.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let top = top
            .into_iter()
            .take(STORM_TOP)
            .map(|(name, n)| format!("{name} x{n}"))
            .collect::<Vec<_>>()
            .join(", ");
        let report = format!(
            "MESSAGE STORM for {:.0} s: {:.0} updates/s, {:.0} views/s; top messages: {}",
            self.high_secs,
            self.updates as f64 / self.window_secs,
            self.views as f64 / self.window_secs,
            if top.is_empty() { "none recorded" } else { &top },
        );
        self.last_report = Some(now);
        self.window_secs = 0.0;
        self.updates = 0;
        self.views = 0;
        Some(report)
    }
}

/// One line per entry, newest last, ages relative to `now`.
fn recent_report(now: u64) -> String {
    let recent = lock(&RECENT);
    let mut out = String::new();
    for (entered, name, took) in recent.iter() {
        let _ = writeln!(
            out,
            "  -{:>6} ms  ({took} ms)  {name}",
            now.saturating_sub(*entered)
        );
    }
    out
}

/// One compact memory line appended to every stall report: process
/// RSS, system availability, swap in use, and the kernel's PSI memory
/// pressure. This is the swap-storm discriminator for #104-class
/// reports: a machine-wide memory stall freezes the app exactly like a
/// render stall (no pixels, no CPU, innocent-looking in-flight
/// message), so a stall line that also says "memory was fine" or
/// "memory was thrashing" keeps us from chasing the named message in
/// the wrong world. Empty when /proc is unavailable; each part is
/// best-effort on its own.
#[cfg(target_os = "linux")]
fn memory_summary() -> String {
    let rss = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| field_kb(&s, "VmRSS:"));
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok();
    let avail = meminfo.as_deref().and_then(|s| field_kb(s, "MemAvailable:"));
    let swap_total = meminfo.as_deref().and_then(|s| field_kb(s, "SwapTotal:"));
    let swap_free = meminfo.as_deref().and_then(|s| field_kb(s, "SwapFree:"));
    // PSI needs CONFIG_PSI (default on modern kernels); absent files
    // just drop the part.
    let psi = std::fs::read_to_string("/proc/pressure/memory")
        .ok()
        .and_then(|s| psi_some_avg10(&s));
    let mut out = String::new();
    if let Some(kb) = rss {
        let _ = write!(out, " rss={}MB", kb / 1024);
    }
    if let Some(kb) = avail {
        let _ = write!(out, " avail={}MB", kb / 1024);
    }
    if let (Some(total), Some(free)) = (swap_total, swap_free) {
        let _ = write!(out, " swap={}/{}MB", (total - free.min(total)) / 1024, total / 1024);
    }
    if let Some(avg10) = psi {
        let _ = write!(out, " psi_mem_some_avg10={avg10}");
    }
    if out.is_empty() { out } else { format!("; mem{out}") }
}

#[cfg(not(target_os = "linux"))]
fn memory_summary() -> String {
    String::new()
}

/// `Key:   123456 kB` -> `123456`, from /proc/self/status or
/// /proc/meminfo shaped text.
#[cfg(any(target_os = "linux", test))]
fn field_kb(text: &str, key: &str) -> Option<u64> {
    text.lines()
        .find_map(|line| line.strip_prefix(key))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse().ok())
}

/// The `avg10` value of the `some` line of a /proc/pressure file:
/// share of the last 10s in which at least one task was stalled on
/// memory, which is the earliest system-wide thrashing signal.
#[cfg(any(target_os = "linux", test))]
fn psi_some_avg10(text: &str) -> Option<String> {
    text.lines()
        .find(|line| line.starts_with("some"))
        .and_then(|line| {
            line.split_whitespace().find_map(|token| token.strip_prefix("avg10="))
        })
        .map(|v| v.to_string())
}

/// The calling thread's kernel id, from `/proc/thread-self` (a link to
/// `<pid>/task/<tid>`).
#[cfg(target_os = "linux")]
fn current_tid() -> Option<u32> {
    std::fs::read_link("/proc/thread-self")
        .ok()?
        .file_name()?
        .to_str()?
        .parse()
        .ok()
}

/// `(state, utime + stime in clock ticks)` from a `/proc/.../stat` line.
/// The command name sits in parentheses and may itself hold spaces and
/// parentheses, so the fields are counted from the LAST `)`: right after
/// it comes field 3 (state), and utime / stime are fields 14 / 15.
#[cfg(any(target_os = "linux", test))]
fn parse_task_stat(text: &str) -> Option<(char, u64)> {
    let rest = &text[text.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let state = fields.first()?.chars().next()?;
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some((state, utime + stime))
}

/// What `/proc/.../syscall` says about a thread.
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, PartialEq, Eq)]
enum SyscallState {
    /// Executing in user space: our code, a library's, a driver's
    /// user-mode half. A spin shows up here.
    Running,
    /// Blocked inside the kernel; the number is the system call (`-1`
    /// when it is blocked outside one, a page fault for instance).
    Blocked(i64),
}

#[cfg(any(target_os = "linux", test))]
fn parse_syscall(text: &str) -> Option<SyscallState> {
    let first = text.split_whitespace().next()?;
    if first == "running" {
        return Some(SyscallState::Running);
    }
    first.parse().ok().map(SyscallState::Blocked)
}

/// One line on the UI thread, appended to every stall report: its state,
/// how much CPU it burned over a 200 ms sample, and whether it runs in
/// user space or waits in the kernel (with the kernel function it waits
/// in). This is what tells a loop of ours from a driver blocking the
/// present, which every other layer reports the same way.
#[cfg(target_os = "linux")]
fn main_thread_summary() -> String {
    let Some(Some(tid)) = MAIN_TID.get().copied() else {
        return String::new();
    };
    let dir = format!("/proc/self/task/{tid}");
    let read = |file: &str| std::fs::read_to_string(format!("{dir}/{file}")).ok();
    let Some((_, ticks_before)) = read("stat").as_deref().and_then(parse_task_stat) else {
        return String::new();
    };
    let sampled = Instant::now();
    std::thread::sleep(Duration::from_millis(200));
    let Some((state, ticks_after)) = read("stat").as_deref().and_then(parse_task_stat) else {
        return String::new();
    };
    // SAFETY: `sysconf` reads a process-wide constant and takes no pointers.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    let hz = if hz > 0 { hz as f64 } else { 100.0 };
    let cpu = ticks_after.saturating_sub(ticks_before) as f64 / hz
        / sampled.elapsed().as_secs_f64().max(0.001)
        * 100.0;
    let doing = match read("syscall").as_deref().and_then(parse_syscall) {
        Some(SyscallState::Running) => "running in user space".to_string(),
        Some(SyscallState::Blocked(nr)) => {
            match read("wchan").map(|w| w.trim().to_string()).filter(|w| !w.is_empty() && w != "0") {
                Some(wchan) => format!("blocked in syscall {nr} (wchan {wchan})"),
                None => format!("blocked in syscall {nr}"),
            }
        }
        None => "syscall state unreadable".to_string(),
    };
    format!("; main thread: state={state} cpu={cpu:.0}% {doing}")
}

#[cfg(not(target_os = "linux"))]
fn main_thread_summary() -> String {
    String::new()
}

/// Spawn the monitor thread once. Parked (cheap sleep loop) while the
/// debug toggle is off, so flipping the setting at runtime arms and
/// disarms it without restarts.
fn ensure_started() {
    if THREAD_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    // Seed the beats so the first poll after enabling doesn't see a
    // process-lifetime "stall".
    let now = now_ms();
    UPDATE_BEAT.store(now, Ordering::Relaxed);
    REAL_BEAT.store(now, Ordering::Relaxed);
    VIEW_BEAT.store(now, Ordering::Relaxed);
    std::thread::Builder::new()
        .name("stall-watchdog".into())
        .spawn(monitor_loop)
        .map(|_| ())
        .unwrap_or_else(|e| {
            tracing::warn!("stall-watchdog: could not spawn monitor thread: {e}");
        });
}

fn monitor_loop() {
    // When the current stall was first logged, so persistence re-logs at
    // STALL_RELOG_MS and recovery is reported once with its duration.
    let mut update_stall_since: Option<u64> = None;
    let mut last_stall_log: u64 = 0;
    let mut view_stall_since: Option<u64> = None;
    let mut diag_armed = false;
    let mut storm = StormMeter::default();
    let mut storm_read_at = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(250));
        let enabled = crate::logging::is_enabled();
        if enabled != diag_armed {
            diag_armed = enabled;
            oryxis_terminal::diag::set_enabled(enabled);
        }
        if !enabled {
            update_stall_since = None;
            view_stall_since = None;
            storm = StormMeter::default();
            continue;
        }
        let now = now_ms();

        // Storm meter, read about once a second.
        let since_read = storm_read_at.elapsed();
        if since_read >= Duration::from_secs(1) {
            storm_read_at = Instant::now();
            let updates = UPDATES.swap(0, Ordering::Relaxed);
            let views = VIEWS.swap(0, Ordering::Relaxed);
            let names = lock(&STORM_NAMES).take().unwrap_or_default();
            if let Some(report) =
                storm.observe(now, since_read.as_secs_f64(), updates, views, names)
            {
                tracing::warn!("stall-watchdog: {report}");
            }
        }
        let update_age = now.saturating_sub(UPDATE_BEAT.load(Ordering::Relaxed));
        let real_age = now.saturating_sub(REAL_BEAT.load(Ordering::Relaxed));
        let view_age = now.saturating_sub(VIEW_BEAT.load(Ordering::Relaxed));

        // Layer 1 + 2: the update loop stopped beating. With the 500 ms
        // pacemaker tick subscribed, a healthy loop can never be this
        // quiet, so this is a real stall wherever it sits.
        if update_age >= STALL_MS {
            let since = *update_stall_since.get_or_insert(now.saturating_sub(update_age));
            if now.saturating_sub(last_stall_log) >= STALL_RELOG_MS {
                last_stall_log = now;
                // Sampled AT the stall, not after: a swap storm and a
                // render stall look identical from inside the process,
                // the memory line is what tells them apart.
                let mem = memory_summary();
                let location = silent_loop_location(now);
                let in_flight = lock(&IN_FLIGHT).clone();
                // Sampled with the lock released: it sleeps 200 ms.
                let main = main_thread_summary();
                match in_flight {
                    Some((entered, name)) => tracing::warn!(
                        "stall-watchdog: UPDATE STALLED for {} ms; handler in flight for {} ms: {name}{main}{mem}\nrecent messages:\n{}",
                        now.saturating_sub(since),
                        now.saturating_sub(entered),
                        recent_report(now),
                    ),
                    None => tracing::warn!(
                        "stall-watchdog: EVENT LOOP SILENT for {} ms with no handler in flight (last view {} ms ago); \
                         the loop is stuck {location}{main}{mem}\nrecent messages:\n{}",
                        now.saturating_sub(since),
                        view_age,
                        recent_report(now),
                    ),
                }
            }
        } else if let Some(since) = update_stall_since.take() {
            last_stall_log = 0;
            tracing::warn!(
                "stall-watchdog: recovered after {} ms of update stall",
                now.saturating_sub(since)
            );
        }

        // Layer 3: messages are being processed but view() stopped
        // running. Gated on REAL messages so an idle app (pacemaker
        // ticks only, iced legitimately skips redraws) never trips it.
        if update_age < STALL_MS && view_age >= VIEW_STALL_MS && real_age < VIEW_STALL_MS {
            if view_stall_since.is_none() {
                view_stall_since = Some(now.saturating_sub(view_age));
                tracing::warn!(
                    "stall-watchdog: PRESENTATION STALLED: update alive (last real message {} ms ago) \
                     but view() has not run for {} ms; iced/wgpu is not drawing{}",
                    real_age,
                    view_age,
                    memory_summary(),
                );
            }
        } else if let Some(since) = view_stall_since.take()
            && view_age < VIEW_STALL_MS
        {
            tracing::warn!(
                "stall-watchdog: presentation recovered after {} ms",
                now.saturating_sub(since)
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_name_truncates_at_budget() {
        // A paste-sized payload must not bloat the ring: the name stops
        // at the budget plus the ellipsis.
        let big = "x".repeat(64 * 1024);
        let msg = crate::app::Message::OpenUrl(big);
        let name = message_name(&msg);
        assert!(name.len() <= NAME_BUDGET + '…'.len_utf8());
        assert!(name.starts_with("OpenUrl"));
        assert!(name.ends_with('…'));
    }

    /// The ring feeds `recent_report`, which is written into the
    /// debug-log file users attach to issues. A message that carries a
    /// secret must reach it redacted: the payload here is the value of
    /// a text input, so an unwrapped one would record the password one
    /// keystroke at a time, ending with the whole thing.
    #[test]
    fn message_name_never_records_a_secret() {
        let msg = crate::app::Message::Vault(crate::app::VaultMessage::VaultPasswordChanged(
            "hunter2".into(),
        ));
        let name = message_name(&msg);
        assert!(!name.contains("hunter2"), "{name}");
        assert!(name.contains("<redacted>"), "{name}");

        // Same for the text that comes back from the system clipboard:
        // pasting a password into a sudo prompt is the everyday case.
        let pasted = crate::app::Message::Terminal(
            crate::app::TerminalMessage::TerminalPasteResolved(
                uuid::Uuid::nil(),
                Some("hunter2".into()),
            ),
        );
        let name = message_name(&pasted);
        assert!(!name.contains("hunter2"), "{name}");
    }

    #[test]
    fn message_name_keeps_short_names_whole() {
        let name = message_name(&crate::app::Message::NoOp);
        assert_eq!(name, "NoOp");
    }

    #[test]
    fn proc_parsers_read_the_fields() {
        let status = "VmPeak:\t  500000 kB\nVmRSS:\t  431104 kB\nThreads:\t30\n";
        assert_eq!(field_kb(status, "VmRSS:"), Some(431_104));
        assert_eq!(field_kb(status, "MemAvailable:"), None);

        let meminfo = "MemTotal:       32000000 kB\nMemAvailable:   9800000 kB\n\
                       SwapTotal:       8388604 kB\nSwapFree:        8388604 kB\n";
        assert_eq!(field_kb(meminfo, "MemAvailable:"), Some(9_800_000));

        let psi = "some avg10=12.34 avg60=5.00 avg300=1.00 total=123456\n\
                   full avg10=3.00 avg60=1.00 avg300=0.10 total=6543\n";
        assert_eq!(psi_some_avg10(psi).as_deref(), Some("12.34"));
        assert_eq!(psi_some_avg10("garbage"), None);
    }

    /// Live read on Linux: the stall reports depend on this never
    /// erroring, whatever the kernel config; at minimum the process's
    /// own RSS must resolve.
    #[test]
    #[cfg(target_os = "linux")]
    fn memory_summary_reads_proc() {
        let mem = memory_summary();
        assert!(mem.starts_with("; mem"), "{mem}");
        assert!(mem.contains("rss="), "{mem}");
    }

    /// The storm tally counts KINDS: two panes and two batches of output
    /// are one entry, and nothing of the payload survives into the log.
    #[test]
    fn message_key_drops_the_payload() {
        assert_eq!(
            message_key("Terminal(PtyOutput(5a1e0273-0000-4000-8000-000000000273, [27, 91, 63, 50]))"),
            "Terminal(PtyOutput(, ))"
        );
        assert_eq!(
            message_key("InWindow(Id(1), Tabs(TabHovered(3)))"),
            "InWindow(Id(), Tabs(TabHovered()))"
        );
        assert_eq!(
            message_key("Tabs(MouseMoved(Point { x: 10.5, y: 20.0 }))"),
            "Tabs(MouseMoved(Point { x: , y:  }))"
        );
        assert_eq!(message_key(r#"OpenUrl("https://example.com/a\"b")"#), "OpenUrl()");
        // Variant names made only of hex letters are names, not ids.
        assert_eq!(message_key("Settings(Add)"), "Settings(Add)");
        // A name cut at the budget ends in an ellipsis, which goes too.
        assert_eq!(message_key("Terminal(PtyOutput(1, [27, 91\u{2026}"), "Terminal(PtyOutput(, ");
        let long = format!("Sftp({})", "VeryLongVariantName".repeat(10));
        assert!(message_key(&long).len() <= 64);
    }

    #[test]
    fn task_stat_is_read_past_a_name_with_spaces_and_parens() {
        // Field 3 is the state, 14 / 15 are utime / stime.
        let line = "4242 (iced (main) x) R 1 4242 4242 0 -1 4194304 100 0 0 0 1500 250 0 0 20 0 30 0 100 0 0";
        assert_eq!(parse_task_stat(line), Some(('R', 1750)));
        assert_eq!(parse_task_stat("4242 (short"), None);
        assert_eq!(parse_task_stat("garbage"), None);
    }

    #[test]
    fn syscall_file_says_running_or_the_call() {
        assert_eq!(parse_syscall("running\n"), Some(SyscallState::Running));
        assert_eq!(
            parse_syscall("7 0x55d 0x1 0xffffffff 0x0 0x0 0x0 0x7ffd 0x7f12\n"),
            Some(SyscallState::Blocked(7))
        );
        assert_eq!(parse_syscall("-1 0x7ffd 0x7f12\n"), Some(SyscallState::Blocked(-1)));
        assert_eq!(parse_syscall(""), None);
    }

    fn names(pairs: &[(&str, u32)]) -> HashMap<String, u32> {
        pairs.iter().map(|(n, c)| ((*n).to_string(), *c)).collect()
    }

    #[test]
    fn storm_meter_reports_a_sustained_rate_once_per_window() {
        let mut m = StormMeter::default();
        // An output flood well under the thresholds never reports.
        for t in 0..10u64 {
            assert_eq!(m.observe(t * 1000, 1.0, 120, 60, names(&[("Terminal(PtyOutput(, ))", 100)])), None);
        }
        // Two high seconds are not enough; the third is.
        assert_eq!(m.observe(20_000, 1.0, 900, 900, names(&[("Tabs(TabHovered())", 800)])), None);
        assert_eq!(m.observe(21_000, 1.0, 900, 900, names(&[("Tabs(TabHovered())", 800)])), None);
        let report = m
            .observe(
                22_000,
                1.0,
                900,
                900,
                names(&[("Tabs(TabHovered())", 800), ("Terminal(PtyOutput(, ))", 90)]),
            )
            .expect("three seconds of storm report");
        assert!(report.starts_with("MESSAGE STORM for 3 s: 900 updates/s, 900 views/s"), "{report}");
        assert!(report.contains("Tabs(TabHovered()) x2400"), "{report}");
        assert!(report.find("TabHovered").unwrap() < report.find("PtyOutput").unwrap(), "{report}");
        // Still storming: quiet until the relog window passes.
        for t in 23..37u64 {
            assert_eq!(m.observe(t * 1000, 1.0, 900, 900, HashMap::new()), None);
        }
        let again = m.observe(37_000, 1.0, 900, 900, HashMap::new()).expect("relogged");
        assert!(again.starts_with("MESSAGE STORM for 18 s: 900 updates/s, 900 views/s"), "{again}");
        // Views alone are a storm too (a tree rebuilt for nothing).
        let mut v = StormMeter::default();
        for t in 0..2u64 {
            assert_eq!(v.observe(t * 1000, 1.0, 10, 500, HashMap::new()), None);
        }
        let report = v.observe(2_000, 1.0, 10, 500, HashMap::new()).expect("views storm");
        assert!(report.ends_with("top messages: none recorded"), "{report}");
        // One kind looping at a frame rate is a storm on its own, while
        // the byte streams at the same rate are not.
        let mut k = StormMeter::default();
        for t in 0..2u64 {
            assert_eq!(k.observe(t * 1000, 1.0, 150, 60, names(&[("Tabs(TabHovered())", 120)])), None);
        }
        let report = k
            .observe(2_000, 1.0, 150, 60, names(&[("Tabs(TabHovered())", 120)]))
            .expect("a single looping kind");
        assert!(report.contains("Tabs(TabHovered()) x360"), "{report}");
        let mut b = StormMeter::default();
        for t in 0..5u64 {
            let flood = names(&[("Terminal(PtyOutput(, ))", 150), ("Terminal(TerminalInput())", 150)]);
            assert_eq!(b.observe(t * 1000, 1.0, 300, 60, flood), None, "byte streams are exempt");
        }
        // A quiet second resets the run.
        let mut q = StormMeter::default();
        q.observe(0, 1.0, 900, 0, HashMap::new());
        q.observe(1_000, 1.0, 900, 0, HashMap::new());
        q.observe(2_000, 1.0, 10, 0, HashMap::new());
        assert_eq!(q.observe(3_000, 1.0, 900, 0, HashMap::new()), None);
    }

    /// A loop that went silent is placed: inside our view, inside the
    /// terminal widget, or outside our code.
    #[test]
    fn silent_loop_location_names_the_view() {
        // The watchdog clock starts near zero in a test process, so the
        // mark is taken now and read 4 s "later".
        let now = now_ms();
        IN_VIEW_SINCE.store(now, Ordering::Relaxed);
        let inside = silent_loop_location(now + 4_000);
        IN_VIEW_SINCE.store(NOT_IN_VIEW, Ordering::Relaxed);
        assert_eq!(inside, "inside view() for 4000 ms");
        assert!(silent_loop_location(now).starts_with("outside Oryxis code"));
    }

    /// Live read on Linux: the summary of a real thread parses, whatever
    /// the kernel config, and says what the thread was doing.
    #[test]
    #[cfg(target_os = "linux")]
    fn main_thread_summary_reads_a_live_thread() {
        MAIN_TID.get_or_init(current_tid);
        let line = main_thread_summary();
        assert!(line.starts_with("; main thread: state="), "{line}");
        assert!(line.contains("cpu="), "{line}");
        assert!(line.contains("running in user space") || line.contains("blocked in syscall"), "{line}");
    }

    #[test]
    fn recent_ring_is_capped() {
        {
            let mut recent = lock(&RECENT);
            recent.clear();
            for i in 0..(RECENT_CAP as u64 + 10) {
                if recent.len() == RECENT_CAP {
                    recent.pop_front();
                }
                recent.push_back((i, format!("m{i}"), 0));
            }
            assert_eq!(recent.len(), RECENT_CAP);
            // Oldest entries were dropped, newest kept.
            assert_eq!(recent.front().unwrap().1, "m10");
        }
        let report = recent_report(now_ms());
        assert_eq!(report.lines().count(), RECENT_CAP);
        assert!(report.contains("m10"));
    }
}
