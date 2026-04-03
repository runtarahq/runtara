//! Database Query Instrumentation
//!
//! Provides utilities for recording database query metrics.
//!
//! # Usage
//!
//! For wrapping individual queries:
//! ```ignore
//! use crate::observability::db::record_query;
//!
//! let result = record_query("get_scenario", async {
//!     sqlx::query!("SELECT * FROM scenarios WHERE id = $1", id)
//!         .fetch_optional(&pool)
//!         .await
//! }).await;
//! ```
//!
//! Or use the macro for simpler syntax:
//! ```ignore
//! use crate::observability::instrument_query;
//!
//! let result = instrument_query!("get_scenario", {
//!     sqlx::query!("SELECT * FROM scenarios WHERE id = $1", id)
//!         .fetch_optional(&pool)
//!         .await
//! });
//! ```

use opentelemetry::KeyValue;
use std::future::Future;
use std::time::Instant;

use super::metrics;

/// Macro for instrumenting database queries with metrics
///
/// # Example
/// ```ignore
/// let user = instrument_query!("get_user_by_id", {
///     sqlx::query_as!(User, "SELECT * FROM users WHERE id = $1", user_id)
///         .fetch_optional(&pool)
///         .await
/// });
/// ```
#[macro_export]
macro_rules! instrument_query {
    ($operation:expr, $query:expr) => {{ $crate::observability::db::record_query($operation, async { $query }).await }};
}

/// Record database query metrics
///
/// Wraps a database query future and records timing and success/failure metrics.
///
/// # Example
///
/// ```ignore
/// use crate::observability::db::record_query;
///
/// let result = record_query("get_scenario", async {
///     sqlx::query!("SELECT * FROM scenarios WHERE id = $1", id)
///         .fetch_optional(&pool)
///         .await
/// }).await;
/// ```
pub async fn record_query<F, T, E>(operation: &str, query: F) -> Result<T, E>
where
    F: Future<Output = Result<T, E>>,
{
    let start = Instant::now();
    let result = query.await;
    let duration = start.elapsed().as_secs_f64();

    if let Some(m) = metrics() {
        let status = if result.is_ok() { "success" } else { "error" };
        let attributes = [
            KeyValue::new("operation", operation.to_string()),
            KeyValue::new("status", status),
        ];

        m.db_queries_total.add(1, &attributes);
        m.db_query_duration.record(
            duration,
            &[KeyValue::new("operation", operation.to_string())],
        );
    }

    result
}

/// Track database pool connection usage
pub fn track_connection_acquired() {
    if let Some(m) = metrics() {
        m.db_pool_connections_active.add(1, &[]);
    }
}

/// Track database pool connection release
pub fn track_connection_released() {
    if let Some(m) = metrics() {
        m.db_pool_connections_active.add(-1, &[]);
    }
}

/// A guard that tracks connection lifetime
pub struct ConnectionGuard;

impl ConnectionGuard {
    pub fn new() -> Self {
        track_connection_acquired();
        Self
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        track_connection_released();
    }
}

impl Default for ConnectionGuard {
    fn default() -> Self {
        Self::new()
    }
}
