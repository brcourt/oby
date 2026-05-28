//! Wrapper-side counters for the top-of-feed status bar.
//!
//! Mirrors what one might check in `top`: how many messages have been
//! received, how many bytes are buffered, how many connections are open,
//! how many errors have occurred, and basic process info (FD count, uptime).
//! Cheap to read each frame: the struct is `Copy`, so the main loop takes
//! a one-instant snapshot and hands it to ratatui without holding the
//! metrics mutex across the draw.

use std::time::{Duration, Instant};

#[derive(Copy, Clone)]
pub struct Metrics {
    started_at: Instant,
    pub entries_received: u64,
    pub updates_received: u64,
    pub updates_orphaned: u64,
    pub agent_connections: u64,
    pub agent_bytes: u64,
    pub accept_errors: u64,
    pub parse_errors: u64,
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            started_at: Instant::now(),
            entries_received: 0,
            updates_received: 0,
            updates_orphaned: 0,
            agent_connections: 0,
            agent_bytes: 0,
            accept_errors: 0,
            parse_errors: 0,
        }
    }
}

impl Metrics {
    pub fn uptime(&self) -> Duration {
        self.started_at.elapsed()
    }
}

/// Best-effort FD count for the current process. macOS exposes them under
/// `/dev/fd`; Linux under `/proc/self/fd`. Returns None if neither is
/// readable (e.g. running in a constrained container).
pub fn fd_count() -> Option<usize> {
    let path = if cfg!(target_os = "macos") {
        "/dev/fd"
    } else {
        "/proc/self/fd"
    };
    std::fs::read_dir(path).ok().map(|e| e.count())
}

pub fn format_bytes(b: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * 1024 * 1024;
    if b >= GB {
        format!("{:.1}G", b as f64 / GB as f64)
    } else if b >= MB {
        format!("{:.1}M", b as f64 / MB as f64)
    } else if b >= KB {
        format!("{:.1}K", b as f64 / KB as f64)
    } else {
        format!("{b}B")
    }
}

pub fn format_uptime(d: Duration) -> String {
    let s = d.as_secs();
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let sec = s % 60;
    if h > 0 {
        format!("{h}h{m:02}m")
    } else if m > 0 {
        format!("{m}m{sec:02}s")
    } else {
        format!("{sec}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bytes_thresholds() {
        assert_eq!(format_bytes(0), "0B");
        assert_eq!(format_bytes(512), "512B");
        assert_eq!(format_bytes(1024), "1.0K");
        assert_eq!(format_bytes(1536), "1.5K");
        assert_eq!(format_bytes(2 * 1024 * 1024), "2.0M");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.0G");
    }

    #[test]
    fn format_uptime_thresholds() {
        assert_eq!(format_uptime(Duration::from_secs(5)), "5s");
        assert_eq!(format_uptime(Duration::from_secs(65)), "1m05s");
        assert_eq!(format_uptime(Duration::from_secs(3661)), "1h01m");
    }
}
