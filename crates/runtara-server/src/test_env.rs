//! Test-only helpers for serialized process-environment mutation.
//!
//! `std::env::set_var`/`remove_var` are process-global and unsafe under the
//! multi-threaded test harness. Every lib test that mutates env vars must
//! hold [`ENV_MUTEX`] for its whole body and restore state via [`EnvGuard`]
//! so tests in *different* modules can't race on shared vars (e.g.
//! `OAUTH2_ISSUER` is read by both the OIDC provider and the discovery
//! handlers). Mirrors the pattern in `runtara-core/src/config.rs`.

use std::env;
use tokio::sync::Mutex;

/// Crate-wide lock serializing all env-mutating lib tests. Async-aware
/// (`tokio::sync::Mutex`) because the tests holding it await handler calls,
/// and holding a std `MutexGuard` across an await trips
/// `clippy::await_holding_lock`.
pub static ENV_MUTEX: Mutex<()> = Mutex::const_new(());

/// Records prior values on `set`/`remove` and restores them (in reverse
/// order) on drop. Must only be used while holding [`ENV_MUTEX`].
pub struct EnvGuard {
    vars: Vec<(String, Option<String>)>,
}

impl EnvGuard {
    pub fn new() -> Self {
        Self { vars: Vec::new() }
    }

    pub fn set(&mut self, key: &str, value: &str) {
        let old = env::var(key).ok();
        self.vars.push((key.to_string(), old));
        // SAFETY: callers hold ENV_MUTEX, so no concurrent env access.
        unsafe { env::set_var(key, value) };
    }

    pub fn remove(&mut self, key: &str) {
        let old = env::var(key).ok();
        self.vars.push((key.to_string(), old));
        // SAFETY: callers hold ENV_MUTEX, so no concurrent env access.
        unsafe { env::remove_var(key) };
    }
}

impl Default for EnvGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.vars.drain(..).rev() {
            // SAFETY: callers hold ENV_MUTEX, so no concurrent env access.
            unsafe {
                match value {
                    Some(v) => env::set_var(&key, v),
                    None => env::remove_var(&key),
                }
            }
        }
    }
}
