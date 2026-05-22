// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Compatibility shims for tracing macros.
//!
//! When `feature = "tracing"` is on (the default for native consumers), the
//! `debug!`/`info!`/`warn!`/`error!` names re-export the real tracing macros.
//! When off (WASI workflow builds), they're local no-op `macro_rules!` stubs
//! that consume their arguments and expand to nothing. Every SDK source file
//! that wants to log writes `use crate::tracing_compat::{info, warn};` and
//! the rest of the code is identical either way.
//!
//! `#[instrument]` cannot be shimmed (it's a proc-macro attribute). Call
//! sites that use it wrap the attribute with `#[cfg_attr(feature = "tracing", instrument(...))]`
//! so it compiles out cleanly when the feature is off.

#[cfg(feature = "tracing")]
#[allow(unused_imports)]
pub(crate) use tracing::{debug, error, info, warn};

#[cfg(not(feature = "tracing"))]
mod stubs {
    // No-op shim that swallows its arguments and expands to a unit expression
    // (`()`). Expanding to `()` lets the macro be used in both statement and
    // expression positions — e.g. `match x { Err(_) => warn!("..."), Ok(_) => {} }` —
    // exactly like the real `tracing::warn!` would.
    //
    // Call sites that bind variables only for use inside the log macro (like
    // `Err(e) => warn!(error = %e, "...")`) will produce `unused_variable`
    // warnings when this no-op is in effect. That's acceptable for the WASI
    // workflow build where this code is silenced; native consumers (default
    // features) get the real tracing macros and the variables are used.
    macro_rules! _noop {
        ($($tt:tt)*) => {{ () }};
    }
    pub(crate) use _noop as debug;
    pub(crate) use _noop as error;
    pub(crate) use _noop as info;
    pub(crate) use _noop as warn;
}

#[cfg(not(feature = "tracing"))]
#[allow(unused_imports)]
pub(crate) use stubs::{debug, error, info, warn};
