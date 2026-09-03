//! Throughput, ETA, and how often the window is told.
//!
//! Separated from the worker because these are the parts with arithmetic in
//! them, and arithmetic that is only ever exercised by moving real bytes is
//! arithmetic nobody checks.

use std::time::{Duration, Instant};

/// How often one transfer may push an update. `xsyncv3`-sized chunks arrive
/// far faster than a window can usefully redraw.
pub const MAX_UPDATE_HZ: u32 = 10;

/// The minimum gap between two updates for one transfer.
pub const MIN_UPDATE_INTERVAL: Duration = Duration::from_millis(1000 / MAX_UPDATE_HZ as u64);

/// Weight given to the newest sample when smoothing the rate.
///
/// A raw instantaneous rate swings wildly — one slow chunk on a shared link
/// halves it, and the ETA built on it jumps between "2 minutes" and "9
/// minutes" while the user watches. Smoothing trades a little responsiveness
/// for a number that can be read.
const SMOOTHING: f64 = 0.3;

/// Tracks throughput and decides when the window should hear about it.
#[derive(Debug)]
pub struct Progress {
    started: Instant,
    last_sample: Instant,
    last_published: Option<Instant>,
    /// `None` until the first sample establishes a baseline.
    ///
    /// It has to be optional. A resumed transfer's first sample is the length
    /// of its `.part` file — a gigabyte that did not just move — and treating
    /// it as a delta from zero reports a gigabyte per second.
    last_bytes: Option<u64>,
    /// Bytes per second, exponentially smoothed.
    rate: Option<f64>,
}

impl Progress {
    /// Begin tracking, with the clock starting now.
    #[must_use]
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            started: now,
            last_sample: now,
            last_published: None,
            last_bytes: None,
            rate: None,
        }
    }

    /// Record cumulative progress, and say whether to publish it.
    ///
    /// `bytes_done` is cumulative rather than a delta, because that is what a
    /// resumed transfer knows: it starts at the length of its `.part` file and
    /// a delta would report the resumed bytes as though they had just moved.
    pub fn sample(&mut self, bytes_done: u64, now: Instant) -> bool {
        match self.last_bytes {
            // The first sample is a baseline, not a measurement. Nothing has
            // been observed moving yet.
            None => {
                self.last_bytes = Some(bytes_done);
                self.last_sample = now;
            }
            Some(previous_bytes) => {
                let elapsed = now.saturating_duration_since(self.last_sample);
                // Two samples in the same instant carry no rate information,
                // and dividing by that interval is how a rate becomes
                // infinity. The baseline is kept, so the next sample measures
                // the whole span rather than losing the bytes in between.
                if elapsed >= Duration::from_millis(1) && bytes_done >= previous_bytes {
                    #[allow(clippy::cast_precision_loss)]
                    let moved = (bytes_done - previous_bytes) as f64;
                    let instant = moved / elapsed.as_secs_f64();
                    self.rate = Some(match self.rate {
                        None => instant,
                        Some(previous) => SMOOTHING.mul_add(instant, (1.0 - SMOOTHING) * previous),
                    });
                    self.last_sample = now;
                    self.last_bytes = Some(bytes_done);
                }
            }
        }

        match self.last_published {
            Some(last) if now.saturating_duration_since(last) < MIN_UPDATE_INTERVAL => false,
            _ => {
                self.last_published = Some(now);
                true
            }
        }
    }

    /// Bytes per second, or `None` before there is anything to say.
    #[must_use]
    pub fn rate(&self) -> Option<u64> {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        self.rate.map(|rate| rate.max(0.0) as u64)
    }

    /// Seconds remaining, or `None` when it cannot be known.
    ///
    /// Unknown when the total is unknown, when nothing has moved yet, or when
    /// the rate has fallen to zero — a stalled transfer has no finite estimate,
    /// and showing a number that grows without bound is worse than showing
    /// none.
    #[must_use]
    pub fn eta_seconds(&self, bytes_done: u64, bytes_total: u64) -> Option<u64> {
        if bytes_total == 0 || bytes_done >= bytes_total {
            return None;
        }
        let rate = self.rate?;
        if rate < 1.0 {
            return None;
        }
        #[allow(clippy::cast_precision_loss)]
        let remaining = (bytes_total - bytes_done) as f64;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Some((remaining / rate).ceil() as u64)
    }

    /// How long this transfer has been running.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
}

impl Default for Progress {
    fn default() -> Self {
        Self::new()
    }
}

/// How long to wait before retrying, doubling per attempt.
///
/// Capped, because an unbounded backoff on a share that comes back after ten
/// minutes means the transfer waits another sixteen for no reason.
#[must_use]
pub fn backoff(attempt: u32) -> Duration {
    const BASE: Duration = Duration::from_secs(1);
    const CAP: Duration = Duration::from_secs(30);
    BASE.saturating_mul(1_u32 << attempt.min(5)).min(CAP)
}

#[cfg(test)]
mod tests {
    use super::{backoff, Progress, MIN_UPDATE_INTERVAL};
    use std::time::{Duration, Instant};

    #[test]
    fn progress_is_published_at_most_ten_times_a_second() {
        // A 1 GiB copy in 1 MiB chunks is a thousand samples. A window told
        // about each would spend more time drawing than the copy takes.
        let mut progress = Progress::new();
        let start = Instant::now();
        let mut published = 0;
        for chunk in 0..1000_u64 {
            // One chunk every millisecond: a thousand samples over a second.
            let now = start + Duration::from_millis(chunk);
            if progress.sample(chunk * 1024, now) {
                published += 1;
            }
        }
        assert!(published <= 11, "published {published} times in one second");
        assert!(published >= 9, "published only {published} times");
    }

    #[test]
    fn the_first_sample_always_publishes() {
        // Otherwise a row sits at "queued" for a tenth of a second after it
        // has visibly started.
        let mut progress = Progress::new();
        assert!(progress.sample(0, Instant::now()));
    }

    #[test]
    fn the_rate_is_smoothed_rather_than_instantaneous() {
        let mut progress = Progress::new();
        let start = Instant::now();
        // A steady megabyte per second...
        for second in 1..=5_u64 {
            progress.sample(second * 1_000_000, start + Duration::from_secs(second));
        }
        let steady = progress.rate().expect("a rate after five samples");
        assert!(
            (900_000..=1_100_000).contains(&steady),
            "steady rate was {steady}"
        );

        // ...then one stalled second. A raw rate would collapse to zero; a
        // smoothed one dips, so the ETA beside it does not jump.
        progress.sample(5_000_000, start + Duration::from_secs(6));
        let dipped = progress.rate().expect("a rate");
        assert!(dipped < steady, "did not react at all");
        assert!(dipped > steady / 2, "collapsed to {dipped} from {steady}");
    }

    #[test]
    fn a_resumed_transfer_does_not_report_its_resumed_bytes_as_throughput() {
        // Progress is cumulative, and a resumed transfer starts at the length
        // of its `.part` file. Treating that first number as a delta would
        // report a gigabyte moved in a millisecond.
        let mut progress = Progress::new();
        let start = Instant::now();
        progress.sample(0, start);
        // The worker seeds the tracker at the resume point before any bytes
        // move, so the first real sample measures only what it moved.
        let mut resumed = Progress::new();
        resumed.sample(1_000_000_000, start);
        resumed.sample(1_000_001_000, start + Duration::from_secs(1));
        let rate = resumed.rate().expect("a rate");
        assert!(rate < 10_000, "reported {rate} B/s after a resume");
    }

    #[test]
    fn eta_is_unknown_rather_than_infinite_when_stalled() {
        let mut progress = Progress::new();
        let start = Instant::now();
        progress.sample(0, start);
        // Nothing moves for ten seconds.
        for second in 1..=10_u64 {
            progress.sample(0, start + Duration::from_secs(second));
        }
        // A number that grows without bound is worse than no number.
        assert_eq!(progress.eta_seconds(0, 1_000_000), None);
    }

    #[test]
    fn eta_is_unknown_when_the_size_is() {
        let mut progress = Progress::new();
        progress.sample(1024, Instant::now());
        assert_eq!(progress.eta_seconds(1024, 0), None);
    }

    #[test]
    fn eta_falls_as_a_transfer_proceeds() {
        let mut progress = Progress::new();
        let start = Instant::now();
        for second in 0..=4_u64 {
            progress.sample(second * 1_000_000, start + Duration::from_secs(second));
        }
        let early = progress.eta_seconds(1_000_000, 10_000_000).expect("eta");
        let late = progress.eta_seconds(9_000_000, 10_000_000).expect("eta");
        assert!(late < early, "{late} was not less than {early}");
    }

    #[test]
    fn backoff_doubles_and_then_stops() {
        assert_eq!(backoff(0), Duration::from_secs(1));
        assert_eq!(backoff(1), Duration::from_secs(2));
        assert_eq!(backoff(2), Duration::from_secs(4));
        // Capped: a share that comes back after ten minutes should not make
        // the transfer wait another sixteen.
        assert_eq!(backoff(20), Duration::from_secs(30));
    }

    #[test]
    fn the_publish_interval_is_the_documented_rate() {
        assert_eq!(MIN_UPDATE_INTERVAL, Duration::from_millis(100));
    }
}
