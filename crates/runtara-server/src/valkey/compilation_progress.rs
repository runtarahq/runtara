//! Compilation Progress
//!
//! Intermediate compilation state lives in Redis with a short TTL. Terminal
//! state (success/failed) lives in `scenario_compilations` in Postgres — this
//! module only tracks what's happening *between* enqueue and DB write so the
//! frontend can render a progress bar.
//!
//! Layout:
//!   KEY:  runtara:compilation:progress:{tenant}:{workflow}:{version}
//!   HASH: stage, stage_index, total_stages, message, started_at, updated_at
//!   TTL:  PROGRESS_TTL_SECS (longer than the longest expected compile)
//!
//! All Redis ops use the shared (non-blocking) `ConnectionManager`.

use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use serde::Serialize;
use std::collections::HashMap;
use tracing::warn;

/// Redis key prefix for compilation progress hashes.
pub const PROGRESS_KEY_PREFIX: &str = "runtara:compilation:progress";

/// TTL on each progress hash. 10 minutes is well past the longest expected
/// compile (cargo-component cold builds top out around 30s in practice) so
/// it auto-cleans without us needing a sweeper.
pub const PROGRESS_TTL_SECS: i64 = 600;

/// User-facing compilation stages, ordered. `stage_index()` and `total()`
/// drive the progress bar fraction on the frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompilationStage {
    Queued,
    Preparing,
    Generating,
    Building,
    Composing,
    Registering,
}

impl CompilationStage {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Preparing => "preparing",
            Self::Generating => "generating",
            Self::Building => "building",
            Self::Composing => "composing",
            Self::Registering => "registering",
        }
    }

    pub fn stage_index(&self) -> u8 {
        match self {
            Self::Queued => 1,
            Self::Preparing => 2,
            Self::Generating => 3,
            Self::Building => 4,
            Self::Composing => 5,
            Self::Registering => 6,
        }
    }

    pub fn total() -> u8 {
        6
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "queued" => Some(Self::Queued),
            "preparing" => Some(Self::Preparing),
            "generating" => Some(Self::Generating),
            "building" => Some(Self::Building),
            "composing" => Some(Self::Composing),
            "registering" => Some(Self::Registering),
            _ => None,
        }
    }
}

/// Snapshot read back for the GET /compilation-progress endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct CompilationProgress {
    pub stage: String,
    pub stage_index: u8,
    pub total_stages: u8,
    pub message: String,
    pub started_at: i64,
    pub updated_at: i64,
}

/// Build the Redis key for a given workflow/version.
fn progress_key(tenant_id: &str, workflow_id: &str, version: i32) -> String {
    format!(
        "{}:{}:{}:{}",
        PROGRESS_KEY_PREFIX, tenant_id, workflow_id, version
    )
}

/// Reporter scoped to a single compilation. Construct one at the start of
/// each `compile_workflow` call; cheap to clone since it just holds a
/// `ConnectionManager` clone (no new TCP).
#[derive(Clone)]
pub struct ProgressReporter {
    manager: ConnectionManager,
    tenant_id: String,
    workflow_id: String,
    version: i32,
    started_at: i64,
}

impl ProgressReporter {
    pub fn new(
        manager: ConnectionManager,
        tenant_id: impl Into<String>,
        workflow_id: impl Into<String>,
        version: i32,
    ) -> Self {
        Self {
            manager,
            tenant_id: tenant_id.into(),
            workflow_id: workflow_id.into(),
            version,
            started_at: chrono::Utc::now().timestamp_millis(),
        }
    }

    fn key(&self) -> String {
        progress_key(&self.tenant_id, &self.workflow_id, self.version)
    }

    /// Write the current stage + message to Redis with a refreshed TTL.
    /// Failures are logged and swallowed — progress reporting must never
    /// break a compile.
    pub async fn report(&self, stage: CompilationStage, message: &str) {
        let mut conn = self.manager.clone();
        let key = self.key();
        let now = chrono::Utc::now().timestamp_millis();

        let fields: [(&str, String); 6] = [
            ("stage", stage.as_str().to_string()),
            ("stage_index", stage.stage_index().to_string()),
            ("total_stages", CompilationStage::total().to_string()),
            ("message", message.to_string()),
            ("started_at", self.started_at.to_string()),
            ("updated_at", now.to_string()),
        ];

        let result: redis::RedisResult<()> = async {
            let _: () = conn.hset_multiple(&key, &fields).await?;
            let _: () = conn.expire(&key, PROGRESS_TTL_SECS).await?;
            Ok(())
        }
        .await;

        if let Err(e) = result {
            warn!(
                error = %e,
                stage = stage.as_str(),
                tenant_id = %self.tenant_id,
                workflow_id = %self.workflow_id,
                version = self.version,
                "Failed to write compilation progress to Redis"
            );
        }
    }

    /// Remove the progress hash. Called when terminal state is recorded in
    /// the DB so polling clients fall through to the DB read.
    pub async fn clear(&self) {
        let mut conn = self.manager.clone();
        let _: redis::RedisResult<()> = conn.del::<_, ()>(self.key()).await;
    }
}

/// Read the current progress hash for a workflow/version. Returns `None` if
/// the key has expired or was never written.
pub async fn read_progress(
    manager: &ConnectionManager,
    tenant_id: &str,
    workflow_id: &str,
    version: i32,
) -> Option<CompilationProgress> {
    let mut conn = manager.clone();
    let key = progress_key(tenant_id, workflow_id, version);

    let map: HashMap<String, String> = match conn.hgetall(&key).await {
        Ok(m) => m,
        Err(e) => {
            warn!(error = %e, "Failed to read compilation progress from Redis");
            return None;
        }
    };

    if map.is_empty() {
        return None;
    }

    Some(CompilationProgress {
        stage: map.get("stage").cloned()?,
        stage_index: map.get("stage_index")?.parse().ok()?,
        total_stages: map.get("total_stages")?.parse().ok()?,
        message: map.get("message").cloned().unwrap_or_default(),
        started_at: map.get("started_at")?.parse().ok()?,
        updated_at: map.get("updated_at")?.parse().ok()?,
    })
}

/// Convenience: write a single "queued" entry. Called from the save handler
/// after the request is successfully enqueued so the frontend's first poll
/// sees a real stage instead of `unknown`.
pub async fn mark_queued(
    manager: &ConnectionManager,
    tenant_id: &str,
    workflow_id: &str,
    version: i32,
) {
    let reporter = ProgressReporter::new(manager.clone(), tenant_id, workflow_id, version);
    reporter
        .report(CompilationStage::Queued, "Waiting for compiler")
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_indices_match_total() {
        let stages = [
            CompilationStage::Queued,
            CompilationStage::Preparing,
            CompilationStage::Generating,
            CompilationStage::Building,
            CompilationStage::Composing,
            CompilationStage::Registering,
        ];
        let total = CompilationStage::total();
        for (i, s) in stages.iter().enumerate() {
            assert_eq!(s.stage_index() as usize, i + 1);
        }
        assert_eq!(stages.len() as u8, total);
    }

    #[test]
    fn stage_string_roundtrip() {
        for s in [
            CompilationStage::Queued,
            CompilationStage::Preparing,
            CompilationStage::Generating,
            CompilationStage::Building,
            CompilationStage::Composing,
            CompilationStage::Registering,
        ] {
            let parsed = CompilationStage::parse(s.as_str()).unwrap();
            assert_eq!(parsed, s);
        }
    }

    #[test]
    fn progress_key_format() {
        let k = progress_key("tenant-x", "wf-y", 7);
        assert_eq!(k, "runtara:compilation:progress:tenant-x:wf-y:7");
    }
}
