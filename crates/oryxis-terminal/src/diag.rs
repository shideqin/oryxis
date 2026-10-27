//! Where the UI thread is inside this crate, for the app's stall
//! watchdog (issue #273).
//!
//! A freeze report that says "the event loop is silent and no handler is
//! in flight" still leaves the whole of iced, the renderer and the driver
//! as suspects. The terminal widget is the one piece of OUR code that
//! runs outside `update()` on every frame (its canvas `draw` and its
//! event handling), so it marks itself here and the watchdog can say
//! "stuck inside the terminal draw for 4 s" instead of "somewhere".
//!
//! Off unless the app arms it ([`set_enabled`], driven by the debug-log
//! switch), so a normal session pays one relaxed load per draw and per
//! event and nothing else.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// What the widget was doing when the watchdog looked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// `canvas::Program::draw`: the grid snapshot and glyph build.
    Draw,
    /// `canvas::Program::update`: an iced event reaching the widget.
    Event,
}

impl Phase {
    fn code(self) -> u8 {
        match self {
            Phase::Draw => 1,
            Phase::Event => 2,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Phase::Draw),
            2 => Some(Phase::Event),
            _ => None,
        }
    }
}

static ENABLED: AtomicBool = AtomicBool::new(false);
/// The phase the UI thread is in, `0` when it is outside the widget.
static PHASE: AtomicU8 = AtomicU8::new(0);
/// When that phase began, in nanoseconds since [`epoch`].
static SINCE_NS: AtomicU64 = AtomicU64::new(0);

fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

fn now_ns() -> u64 {
    u64::try_from(epoch().elapsed().as_nanos()).unwrap_or(u64::MAX)
}

/// Arm or disarm the marks. The app calls this with its debug-log switch.
pub fn set_enabled(on: bool) {
    if on {
        // Pin the epoch before the first mark so no mark predates it.
        let _ = epoch();
    }
    ENABLED.store(on, Ordering::Relaxed);
    if !on {
        PHASE.store(0, Ordering::Relaxed);
    }
}

/// The phase the UI thread is in right now and for how long, `None` when
/// it is outside the widget (or the marks are off).
pub fn in_flight() -> Option<(Phase, Duration)> {
    if !ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    let phase = Phase::from_code(PHASE.load(Ordering::Acquire))?;
    let since = SINCE_NS.load(Ordering::Relaxed);
    Some((phase, Duration::from_nanos(now_ns().saturating_sub(since))))
}

/// Marks a phase for as long as it lives, and puts back whatever was
/// marked before it (a draw never nests inside an event, but a guard that
/// restores is right whatever the caller does).
pub(crate) struct PhaseGuard {
    armed: bool,
    prev_phase: u8,
    prev_since: u64,
}

pub(crate) fn enter(phase: Phase) -> PhaseGuard {
    if !ENABLED.load(Ordering::Relaxed) {
        return PhaseGuard { armed: false, prev_phase: 0, prev_since: 0 };
    }
    let prev_phase = PHASE.load(Ordering::Relaxed);
    let prev_since = SINCE_NS.load(Ordering::Relaxed);
    SINCE_NS.store(now_ns(), Ordering::Relaxed);
    PHASE.store(phase.code(), Ordering::Release);
    PhaseGuard { armed: true, prev_phase, prev_since }
}

impl Drop for PhaseGuard {
    fn drop(&mut self) {
        if self.armed {
            SINCE_NS.store(self.prev_since, Ordering::Relaxed);
            PHASE.store(self.prev_phase, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One test owns the process-wide marks, so the steps run in order.
    #[test]
    fn marks_follow_the_guard_and_stay_off_when_disarmed() {
        set_enabled(false);
        {
            let _g = enter(Phase::Draw);
            assert_eq!(in_flight(), None, "disarmed marks report nothing");
        }

        set_enabled(true);
        assert_eq!(in_flight(), None);
        {
            let _draw = enter(Phase::Draw);
            let (phase, _) = in_flight().expect("inside a draw");
            assert_eq!(phase, Phase::Draw);
            {
                let _event = enter(Phase::Event);
                assert_eq!(in_flight().map(|(p, _)| p), Some(Phase::Event));
            }
            // The inner guard puts the outer phase back.
            assert_eq!(in_flight().map(|(p, _)| p), Some(Phase::Draw));
        }
        assert_eq!(in_flight(), None, "leaving the widget clears the mark");

        // A guard taken while armed still restores after a disarm.
        let g = enter(Phase::Event);
        set_enabled(false);
        drop(g);
        set_enabled(true);
        assert_eq!(in_flight(), None);
        set_enabled(false);
    }
}
