// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! How this crate reads its settings, and the rules every reader follows.
//!
//! Two things live here. The parsers, so that a setting spelled the same way in
//! two families of variable is read the same way in both — the crate used to
//! have three `*_CLEANUP_POLL_INTERVAL_SECS` variables whose zero values meant
//! three different things. And [`Vars`], the seam that lets a `from_env`
//! constructor be tested against values a test supplies rather than against
//! process-global state every other test in the binary shares.
//!
//! Nothing here reads the environment on its own; [`ProcessEnv`] is the only
//! thing that does, and it is one line.

use std::time::Duration;

/// Where a configuration value comes from.
///
/// The point of the indirection is testing. `std::env::set_var` is process-wide
/// and `unsafe` since the 2024 edition, so a test that sets a variable to check
/// how a config parses it is racing every other test in the binary. Readers in
/// this crate that used to be untestable for that reason grew a
/// `*_from_raw` twin each, which worked but meant the tested function and the
/// called one were never quite the same function. Taking the lookup as an
/// argument tests the real one.
pub(crate) trait Vars {
    /// The value set for `key`, if any.
    fn get(&self, key: &str) -> Option<String>;
}

/// The real environment. The only thing in this crate that reads it.
pub(crate) struct ProcessEnv;

impl Vars for ProcessEnv {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

/// A fixed set of values, so a test can say what the environment holds.
#[cfg(test)]
pub(crate) struct FixedVars(pub(crate) std::collections::HashMap<String, String>);

#[cfg(test)]
impl FixedVars {
    /// Build from `(key, value)` pairs.
    pub(crate) fn new<const N: usize>(pairs: [(&str, &str); N]) -> Self {
        Self(
            pairs
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    /// An environment with nothing set in it.
    pub(crate) fn empty() -> Self {
        Self(std::collections::HashMap::new())
    }
}

#[cfg(test)]
impl Vars for FixedVars {
    fn get(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
}

/// Parse an optional enabled value using the default-on policy.
///
/// Only `false`, `0`, `no`, `off`, or `disabled` (trimmed, case-insensitive)
/// disable the feature. Missing values, empty strings, and typos leave it enabled.
/// The caller supplies the value; this function never reads the environment.
pub fn parse_enabled(value: Option<&str>) -> bool {
    !value.is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "false" | "0" | "no" | "off" | "disabled"
        )
    })
}

/// Parse a setting that has to be positive, falling back to `default` for
/// anything absent, unparseable, or non-positive.
///
/// This is the crate's rule for every interval, batch size, age and concurrency
/// it reads, because honouring a zero is worse than ignoring it. A zero poll
/// interval spins a worker as fast as the database will answer. A zero batch
/// size makes a sweep delete `LIMIT 0` rows forever, because the loop only
/// stops when a pass comes back short of a full batch and `0 < 0` never does. A
/// zero concurrency stops a scheduler doing any work at all, and a zero
/// retention age deletes everything the first time it runs. Each of those
/// hammers something or breaks shutdown, and none is a value an operator can
/// usefully have meant.
///
/// A setting where zero *is* meaningful does not belong here — see
/// `debug_event_max_age_from_raw`, where zero means "do not sweep" and says so.
pub(crate) fn positive_or_default<T>(raw: Option<&str>, default: T) -> T
where
    T: std::str::FromStr + PartialOrd + Default + Copy,
{
    raw.and_then(|v| v.trim().parse::<T>().ok())
        .filter(|parsed| *parsed > T::default())
        .unwrap_or(default)
}

/// Read a positive setting straight from `vars`.
///
/// The [`positive_or_default`] rule, spelled once for the common case of "look
/// this key up and apply it".
pub(crate) fn positive<T>(vars: &dyn Vars, key: &str, default: T) -> T
where
    T: std::str::FromStr + PartialOrd + Default + Copy,
{
    positive_or_default(vars.get(key).as_deref(), default)
}

/// A whole number of days, as a [`Duration`].
pub(crate) fn days(count: u64) -> Duration {
    Duration::from_secs(count * 24 * 3600)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_unless_explicitly_disabled() {
        assert!(parse_enabled(None));
        for value in [
            "true",
            "1",
            "yes",
            "on",
            "True",
            "TRUE",
            "anything-else",
            "",
            "  true  ",
        ] {
            assert!(parse_enabled(Some(value)), "{value:?}");
        }
        for value in [
            "false",
            "0",
            "no",
            "off",
            "disabled",
            "FALSE",
            "Off",
            "  false  ",
        ] {
            assert!(!parse_enabled(Some(value)), "{value:?}");
        }
    }

    #[test]
    fn a_positive_setting_is_taken_and_anything_else_falls_back() {
        assert_eq!(positive_or_default(Some("7"), 3u64), 7);
        assert_eq!(positive_or_default(Some("  7  "), 3u64), 7, "values trim");

        for value in ["0", "-1", "", "   ", "abc", "3.5", "0x10"] {
            assert_eq!(
                positive_or_default(Some(value), 3u64),
                3,
                "{value:?} must not be honoured"
            );
        }
        assert_eq!(positive_or_default(None, 3u64), 3);
    }

    #[test]
    fn positive_reads_through_the_var_source() {
        let vars = FixedVars::new([("A", "9"), ("B", "0")]);
        assert_eq!(positive(&vars, "A", 3u64), 9);
        assert_eq!(positive(&vars, "B", 3u64), 3, "zero falls back");
        assert_eq!(positive(&vars, "MISSING", 3u64), 3);
    }

    #[test]
    fn days_are_whole_days() {
        assert_eq!(days(0), Duration::ZERO);
        assert_eq!(days(3), Duration::from_secs(259_200));
    }

    /// The three cleanup workers must read the same setting the same way.
    ///
    /// They did not. All three take a poll interval in seconds and a maximum
    /// age in days under parallel names, but only the database worker ran them
    /// through the positive-only rule; the image and run-directory workers took
    /// whatever parsed. So `…_POLL_INTERVAL_SECS=0` fell back to the documented
    /// default for one worker and spun the other two, and `…_MAX_AGE_DAYS=0`
    /// was ignored by one and told the other two that everything on disk was
    /// old enough to delete. Nothing announced the difference — each worker
    /// simply started and reported the interval it had settled on.
    #[test]
    fn the_three_cleanup_workers_agree_on_a_zero() {
        let run_dir = crate::cleanup_worker::CleanupWorkerConfig::from_vars(&FixedVars::new([
            ("RUNTARA_RUN_DIR_CLEANUP_POLL_INTERVAL_SECS", "0"),
            ("RUNTARA_RUN_DIR_CLEANUP_MAX_AGE_DAYS", "0"),
        ]));
        let image =
            crate::image_cleanup_worker::ImageCleanupWorkerConfig::from_vars(&FixedVars::new([
                ("RUNTARA_IMAGE_CLEANUP_POLL_INTERVAL_SECS", "0"),
                ("RUNTARA_IMAGE_CLEANUP_MAX_AGE_DAYS", "0"),
            ]));
        let db = crate::db_cleanup_worker::DbCleanupWorkerConfig::from_vars(&FixedVars::new([
            ("RUNTARA_DB_CLEANUP_POLL_INTERVAL_SECS", "0"),
            ("RUNTARA_DB_CLEANUP_MAX_AGE_DAYS", "0"),
        ]));

        for (worker, interval, max_age) in [
            ("run-dir", run_dir.poll_interval, run_dir.max_age),
            ("image", image.poll_interval, image.max_age),
            ("db", db.poll_interval, db.max_age),
        ] {
            assert!(
                !interval.is_zero(),
                "{worker} cleanup took a zero poll interval, which spins its loop"
            );
            assert!(
                !max_age.is_zero(),
                "{worker} cleanup took a zero retention age, which deletes everything"
            );
        }

        // Each still keeps its own default rather than being flattened onto a
        // shared one — the rule is shared, the numbers are not.
        assert_eq!(run_dir.poll_interval, Duration::from_secs(3600));
        assert_eq!(image.poll_interval, Duration::from_secs(6 * 3600));
        assert_eq!(db.poll_interval, Duration::from_secs(3600));
        for max_age in [run_dir.max_age, image.max_age, db.max_age] {
            assert_eq!(max_age, days(3));
        }
    }

    /// Defaults with nothing set, which is what most deployments run.
    #[test]
    fn an_empty_environment_gives_every_worker_its_documented_defaults() {
        let run_dir = crate::cleanup_worker::CleanupWorkerConfig::from_vars(&FixedVars::empty());
        let image =
            crate::image_cleanup_worker::ImageCleanupWorkerConfig::from_vars(&FixedVars::empty());
        let db = crate::db_cleanup_worker::DbCleanupWorkerConfig::from_vars(&FixedVars::empty());

        assert!(run_dir.enabled && image.enabled && db.enabled);
        assert_eq!(run_dir.poll_interval, Duration::from_secs(3600));
        assert_eq!(image.poll_interval, Duration::from_secs(6 * 3600));
        assert_eq!(image.batch_size, 50);
        assert_eq!(db.poll_interval, Duration::from_secs(3600));
        assert_eq!(db.batch_size, 100);
        assert_eq!(db.debug_event_max_age, Some(Duration::from_secs(24 * 3600)));
    }
}
