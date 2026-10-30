//! How many events a launch may append through `POST /v1/events`, and how
//! fast. Each launch has a budget of events and of bytes that refills at a
//! steady rate. A batch over budget is held until the budget has refilled
//! enough for it, so a run that emits a lot is slowed, not failed; a batch that
//! would wait longer than [`MAX_HOLD`] is refused as `capacity`.

use std::time::{Duration, Instant};

/// Events a launch appends per second, sustained.
pub const EVENTS_PER_SECOND: f64 = 1_000.0;
/// Events a launch may append at once, from a full budget.
pub const EVENT_BURST: f64 = 10_000.0;
/// Event bytes a launch appends per second, sustained.
pub const EVENT_BYTES_PER_SECOND: f64 = 4.0 * 1024.0 * 1024.0;
/// Event bytes a launch may append at once, from a full budget.
pub const EVENT_BYTE_BURST: f64 = 32.0 * 1024.0 * 1024.0;
/// The longest a batch is held for the budget before it is refused.
pub const MAX_HOLD: Duration = Duration::from_secs(10);

/// One launch's event budget.
#[derive(Debug)]
pub struct EventBudget {
    events: Bucket,
    bytes: Bucket,
}

impl EventBudget {
    pub fn new(now: Instant) -> Self {
        Self {
            events: Bucket::new(EVENTS_PER_SECOND, EVENT_BURST, now),
            bytes: Bucket::new(EVENT_BYTES_PER_SECOND, EVENT_BYTE_BURST, now),
        }
    }

    /// Takes a batch of `events` events of `bytes` bytes from the budget.
    /// `Ok(wait)`: the batch is admitted once `wait` has passed (zero when the
    /// budget holds it). `Err(wait)`: it would wait longer than [`MAX_HOLD`],
    /// and nothing was taken.
    pub fn take(
        &mut self,
        events: usize,
        bytes: usize,
        now: Instant,
    ) -> Result<Duration, Duration> {
        self.events.refill(now);
        self.bytes.refill(now);
        let wait = self
            .events
            .wait_for(events as f64)
            .max(self.bytes.wait_for(bytes as f64));
        if wait > MAX_HOLD {
            return Err(wait);
        }
        self.events.level -= events as f64;
        self.bytes.level -= bytes as f64;
        Ok(wait)
    }
}

/// A budget that refills at `rate` per second up to `burst`. Its level goes
/// below zero for a batch held until it refills, so batches held at once wait
/// in turn.
#[derive(Debug)]
struct Bucket {
    rate: f64,
    burst: f64,
    level: f64,
    at: Instant,
}

impl Bucket {
    fn new(rate: f64, burst: f64, now: Instant) -> Self {
        Self {
            rate,
            burst,
            level: burst,
            at: now,
        }
    }

    fn refill(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.at).as_secs_f64();
        self.level = (self.level + elapsed * self.rate).min(self.burst);
        self.at = now;
    }

    /// How long until `amount` is available once taken.
    fn wait_for(&self, amount: f64) -> Duration {
        let short = amount - self.level;
        if short <= 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(short / self.rate)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_passes_then_batches_wait_their_turn() {
        let start = Instant::now();
        let mut budget = EventBudget::new(start);
        assert_eq!(budget.take(10_000, 1, start), Ok(Duration::ZERO));
        // Empty: the next 1,000 wait a second, and the 1,000 after them two.
        assert_eq!(budget.take(1_000, 1, start), Ok(Duration::from_secs(1)));
        assert_eq!(budget.take(1_000, 1, start), Ok(Duration::from_secs(2)));
        // A second later the first of those is due, and the budget refilled.
        let later = start + Duration::from_secs(1);
        assert_eq!(budget.take(1_000, 1, later), Ok(Duration::from_secs(2)));
    }

    #[test]
    fn a_batch_that_would_wait_too_long_is_refused_and_takes_nothing() {
        let start = Instant::now();
        let mut budget = EventBudget::new(start);
        assert_eq!(budget.take(10_000, 0, start), Ok(Duration::ZERO));
        assert!(budget.take(9_000, 0, start).is_ok(), "9 s");
        assert_eq!(
            budget.take(2_000, 0, start),
            Err(Duration::from_secs(11)),
            "11 s is over the hold"
        );
        assert_eq!(budget.take(1_000, 0, start), Ok(Duration::from_secs(10)));
    }

    #[test]
    fn bytes_are_budgeted_too() {
        let start = Instant::now();
        let mut budget = EventBudget::new(start);
        let mib = 1024 * 1024;
        assert_eq!(budget.take(1, 32 * mib, start), Ok(Duration::ZERO));
        assert_eq!(budget.take(1, 4 * mib, start), Ok(Duration::from_secs(1)));
        assert!(budget.take(1, 40 * mib, start).is_err());
    }

    #[test]
    fn an_honest_runs_pace_is_never_held() {
        // Two step events of 4 KiB each, 300 steps a second, for a minute.
        let start = Instant::now();
        let mut budget = EventBudget::new(start);
        for n in 0..18_000u64 {
            let now = start + Duration::from_micros(n * 3_334);
            assert_eq!(
                budget.take(2, 8 * 1024, now),
                Ok(Duration::ZERO),
                "step {n}"
            );
        }
    }
}
