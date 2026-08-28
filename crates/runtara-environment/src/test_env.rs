// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Environment-variable guard shared by every test in this crate.
//!
//! The environment is process-wide, and so is the lock that protects it: one
//! guard for the crate, never one per module. Two modules with a mutex each do
//! not serialize against one another at all, which is the failure this module
//! exists to make impossible — `set_var` is `unsafe` precisely because a
//! concurrent `getenv` elsewhere in the process can read memory the write just
//! freed.

use std::env;
use std::sync::{Mutex, MutexGuard};

static ENV_MUTEX: Mutex<()> = Mutex::new(());

/// Sets environment variables for the lifetime of a test and restores their
/// previous values on drop — including when the test panics part-way through.
///
/// Constructing one takes the crate-wide env lock, so holding a guard *is*
/// what serializes a test against the others; there is no separate lock to
/// remember to take.
pub(crate) struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    vars: Vec<(String, Option<String>)>,
}

impl EnvGuard {
    /// Acquire the env lock. Returns once this test has exclusive use of the
    /// process environment.
    pub(crate) fn new() -> Self {
        Self {
            // A test that panicked while holding the lock poisons it. The
            // remaining tests still need serializing, and this guard restores
            // whatever it finds, so recover the inner guard instead of turning
            // one failure into a cascade of unrelated ones.
            _lock: ENV_MUTEX
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            vars: Vec::new(),
        }
    }

    /// Set a variable, remembering what it held before.
    pub(crate) fn set(&mut self, key: &str, value: &str) {
        self.remember(key);
        // SAFETY: this guard holds ENV_MUTEX, and every test in this crate
        // mutates the environment through it, so no other thread is reading.
        unsafe { env::set_var(key, value) };
    }

    /// Unset a variable, remembering what it held before.
    pub(crate) fn remove(&mut self, key: &str) {
        self.remember(key);
        // SAFETY: as in `set`.
        unsafe { env::remove_var(key) };
    }

    fn remember(&mut self, key: &str) {
        let old = env::var(key).ok();
        self.vars.push((key.to_string(), old));
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.vars.drain(..).rev() {
            // SAFETY: the lock is still held — it is dropped after this.
            unsafe {
                match value {
                    Some(v) => env::set_var(&key, v),
                    None => env::remove_var(&key),
                }
            }
        }
    }
}
