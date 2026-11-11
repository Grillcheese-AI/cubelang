//! The call governor (2026-10-24): `@ratelimit` windows and the `@cron` scheduler's clock.
//!
//! Kept OUTSIDE a single run's state on purpose. `run-proto` builds a fresh `VM` per request, so a
//! limit held inside the VM would reset on every request and limit nothing. The resident host keeps
//! one `Governor` and hands it to each request's VM (`VM::governor`), then takes it back; the
//! one-shot `run` gets a fresh one, which is all a one-call process can mean.
//!
//! Times are `Instant`s passed in by the caller, so the rules are testable without sleeping.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

#[derive(Debug, Default, Clone)]
pub struct Governor {
    /// `program.function` -> the instants of its admitted calls inside the current window.
    calls: HashMap<String, VecDeque<Instant>>,
    /// `program.function` -> when the scheduler last ran it.
    cron_last: HashMap<String, Instant>,
}

impl Governor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Admit one call to `key` at `now` when fewer than `n` were admitted in the last `period`, and
    /// record it. Otherwise refuse it, unrecorded, with the wait until the oldest call leaves the
    /// window: a refused call does not push the window forward.
    pub fn admit(&mut self, key: &str, n: u32, period: Duration, now: Instant) -> Result<(), String> {
        let q = self.calls.entry(key.to_string()).or_default();
        while let Some(&t) = q.front() {
            if now.saturating_duration_since(t) >= period {
                q.pop_front();
            } else {
                break;
            }
        }
        if q.len() as u32 >= n {
            let oldest = *q.front().expect("n >= 1, so a full window is non-empty");
            let wait = period.saturating_sub(now.saturating_duration_since(oldest));
            return Err(format!(
                "{} is @ratelimit({}, \"{}\"): {} call(s) in the last {}; the next is admitted in {} ms",
                key, n, fmt_period(period), q.len(), fmt_period(period), wait.as_millis()
            ));
        }
        q.push_back(now);
        Ok(())
    }

    /// Calls admitted for `key` in the window ending at `now` (for reports and tests).
    pub fn in_window(&self, key: &str, period: Duration, now: Instant) -> usize {
        self.calls.get(key).map_or(0, |q| {
            q.iter().filter(|&&t| now.saturating_duration_since(t) < period).count()
        })
    }

    /// Is the `@cron` function `key` due at `now`? A function that never ran is due.
    pub fn due(&self, key: &str, interval: Duration, now: Instant) -> bool {
        match self.cron_last.get(key) {
            None => true,
            Some(&t) => now.saturating_duration_since(t) >= interval,
        }
    }

    pub fn mark_ran(&mut self, key: &str, now: Instant) {
        self.cron_last.insert(key.to_string(), now);
    }
}

/// `"10ms"`, `"30s"`, `"1m"`, `"1h"`, `"24h"`, `"7d"`; a bare number is seconds. Zero is refused: a
/// zero window limits nothing, and a zero interval would run a cron on every tick, which is the
/// same as calling it and not a schedule.
pub fn parse_period(s: &str) -> Result<Duration, String> {
    let t = s.trim();
    let split = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
    let (num, unit) = t.split_at(split);
    let n: u64 = num.parse().map_err(|_| format!("period {:?}: expected a number then a unit (ms, s, m, h, d)", s))?;
    let ms = match unit.trim() {
        "ms" => n,
        "" | "s" => n * 1_000,
        "m" => n * 60_000,
        "h" => n * 3_600_000,
        "d" => n * 86_400_000,
        other => return Err(format!("period {:?}: unknown unit {:?} (ms, s, m, h, d)", s, other)),
    };
    if ms == 0 {
        return Err(format!("period {:?} is zero", s));
    }
    Ok(Duration::from_millis(ms))
}

/// The shortest exact spelling of a period: `60000 ms` is `1m`.
pub fn fmt_period(d: Duration) -> String {
    let ms = d.as_millis() as u64;
    for (unit, size) in [("d", 86_400_000u64), ("h", 3_600_000), ("m", 60_000), ("s", 1_000)] {
        if ms % size == 0 {
            return format!("{}{}", ms / size, unit);
        }
    }
    format!("{}ms", ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periods_parse_and_print_back() {
        assert_eq!(parse_period("10ms").unwrap(), Duration::from_millis(10));
        assert_eq!(parse_period("1m").unwrap(), Duration::from_secs(60));
        assert_eq!(parse_period("24h").unwrap(), Duration::from_secs(86_400));
        assert_eq!(parse_period("5").unwrap(), Duration::from_secs(5));
        assert!(parse_period("0s").is_err());
        assert!(parse_period("fast").is_err());
        assert!(parse_period("3w").is_err());
        assert_eq!(fmt_period(Duration::from_secs(3600)), "1h");
        assert_eq!(fmt_period(Duration::from_millis(1500)), "1500ms");
    }

    #[test]
    fn a_window_admits_n_then_refuses_until_the_oldest_leaves() {
        let mut g = Governor::new();
        let t0 = Instant::now();
        let p = Duration::from_secs(60);
        for i in 0..3 {
            assert!(g.admit("P.f", 3, p, t0 + Duration::from_secs(i)).is_ok());
        }
        let refused = g.admit("P.f", 3, p, t0 + Duration::from_secs(10)).unwrap_err();
        assert!(refused.contains("@ratelimit(3, \"1m\")") && refused.contains("50000 ms"), "{}", refused);
        assert_eq!(g.in_window("P.f", p, t0 + Duration::from_secs(10)), 3, "a refusal is not recorded");
        assert!(g.admit("P.f", 3, p, t0 + Duration::from_secs(60)).is_ok(), "the first call left the window");
        assert!(g.admit("Q.f", 3, p, t0).is_ok(), "windows are per function");
    }

    #[test]
    fn a_cron_is_due_first_then_once_per_interval() {
        let mut g = Governor::new();
        let t0 = Instant::now();
        let i = Duration::from_secs(10);
        assert!(g.due("P.tick", i, t0));
        g.mark_ran("P.tick", t0);
        assert!(!g.due("P.tick", i, t0 + Duration::from_secs(9)));
        assert!(g.due("P.tick", i, t0 + Duration::from_secs(10)));
    }
}
