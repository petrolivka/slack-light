//! The numbers. Everything the spike plan asks to be *measured* rather than
//! judged is collected here and printed to stdout as `key=value` lines, so a
//! run is a record and not an impression.
//!
//! Three sources, none of them a library: `/proc/self/status` for resident
//! memory, `/proc/self/schedstat` for CPU time in nanoseconds, and the
//! window's `gdk::FrameClock` for frame-to-frame intervals while the list is
//! scrolled programmatically. Wall-clock marks are `Instant`s from `main`.

use gtk::prelude::*;
use std::cell::RefCell;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub bench: bool,
    pub idle_secs: u64,
    /// Send this text once the conversation has loaded, through the same
    /// path the composer uses, and time the optimistic row and its
    /// confirmation. A6 as a number rather than a keystroke.
    pub send: Option<String>,
    /// Scroll to this row once loaded, for a screenshot of a particular
    /// message rather than of the end.
    pub jump: Option<u32>,
}

static START: OnceLock<Instant> = OnceLock::new();

pub fn mark_start(t: Instant) {
    let _ = START.set(t);
}

pub fn since_start() -> Duration {
    START.get().map(|t| t.elapsed()).unwrap_or_default()
}

fn status_kb(key: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with(key))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(0)
}

/// The resident set split three ways. `RssAnon` is what this process
/// allocated for itself; `RssFile` is mapped libraries — GTK, Mesa, fonts —
/// shared with every other GTK process on the desktop and counted again in
/// each of them; `RssShmem` is shared memory, mostly the GPU driver's.
/// "How much memory does the client use" has three honest answers, and
/// NFR-4 has to say which one it means.
pub fn report_memory(prefix: &str) {
    report(&format!("{prefix}_rss_kb"), status_kb("VmRSS:"));
    report(&format!("{prefix}_anon_kb"), status_kb("RssAnon:"));
    report(&format!("{prefix}_file_kb"), status_kb("RssFile:"));
    report(&format!("{prefix}_shmem_kb"), status_kb("RssShmem:"));
}

/// CPU time consumed so far, in nanoseconds, every thread included.
pub fn cpu_ns() -> u64 {
    std::fs::read_to_string("/proc/self/schedstat")
        .ok()
        .and_then(|s| s.split_whitespace().next().and_then(|v| v.parse().ok()))
        .unwrap_or(0)
}

pub fn report(key: &str, value: impl std::fmt::Display) {
    println!("{key}={value}");
}

/// Frame intervals recorded from a frame clock, in microseconds.
#[derive(Default)]
pub struct Frames {
    last: Option<i64>,
    intervals: Vec<i64>,
}

thread_local! {
    static FRAMES: RefCell<Frames> = RefCell::new(Frames::default());
}

/// Start recording every paint of `window`. Called once the window is
/// mapped, which is the first moment it has a frame clock.
pub fn record_frames(window: &gtk::Window) {
    let Some(clock) = window.frame_clock() else {
        return;
    };
    clock.connect_after_paint(|clock| {
        let now = clock.frame_time();
        FRAMES.with(|f| {
            let mut f = f.borrow_mut();
            if let Some(prev) = f.last {
                f.intervals.push(now - prev);
            }
            f.last = Some(now);
        });
    });
}

pub fn reset_frames() {
    FRAMES.with(|f| {
        let mut f = f.borrow_mut();
        f.last = None;
        f.intervals.clear();
    });
}

/// Summarise what was recorded: count, p50, p95, max, and the share of
/// frames that missed a 60 Hz budget. Percentiles rather than a mean,
/// because one slow frame is what the eye notices and a mean hides it.
pub fn frame_summary(prefix: &str) {
    FRAMES.with(|f| {
        let f = f.borrow();
        let mut v: Vec<i64> = f
            .intervals
            .iter()
            .copied()
            // A gap over a second is the clock idling between phases, not a
            // frame that took a second.
            .filter(|&d| d < 1_000_000)
            .collect();
        if v.is_empty() {
            report(&format!("{prefix}_frames"), 0);
            return;
        }
        v.sort_unstable();
        let pct = |p: f64| v[((v.len() - 1) as f64 * p) as usize];
        let missed = v.iter().filter(|&&d| d > 20_000).count();
        report(&format!("{prefix}_frames"), v.len());
        report(
            &format!("{prefix}_p50_ms"),
            format!("{:.1}", pct(0.50) as f64 / 1000.0),
        );
        report(
            &format!("{prefix}_p95_ms"),
            format!("{:.1}", pct(0.95) as f64 / 1000.0),
        );
        report(
            &format!("{prefix}_max_ms"),
            format!("{:.1}", v[v.len() - 1] as f64 / 1000.0),
        );
        report(
            &format!("{prefix}_over_20ms_pct"),
            format!("{:.1}", 100.0 * missed as f64 / v.len() as f64),
        );
    });
}
