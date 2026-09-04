// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Durable, generation-scoped launch scheduling.
//!
//! An [`Launch`] is deliberately separate from a Core instance: an instance
//! may survive many physical attempts as it parks and wakes, while a launch is
//! one bounded handoff to a runner.  The queue is the durable replacement for
//! waiting on a runner semaphore in a request, trigger, or wake task.

use std::{collections::HashMap, time::Duration};

use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use thiserror::Error;

/// Stable failure detail written when a launch exceeds its queue deadline.
pub const LAUNCH_QUEUE_TIMEOUT: &str = "launch_queue_timeout";

/// Stable detail recorded when a dispatcher found the runner full.
pub const RUNNER_CAPACITY_UNAVAILABLE: &str = "runner_capacity_unavailable";

/// Stable detail recorded when bounded artifact/component preparation outlived
/// its dispatcher lease. The launch is retried from the durable queue rather
/// than leaving an unbounded compiler task associated with a runner permit.
pub const PREPARATION_TIMEOUT: &str = "preparation_timeout";

/// Stable detail recorded when every independent preparation worker is busy.
pub const PREPARATION_CAPACITY_UNAVAILABLE: &str = "preparation_capacity_unavailable";

/// Stable refusal detail for a trigger whose workflow already has active work.
pub const SINGLE_INSTANCE_ACTIVE: &str = "single_instance_active";

/// Reserved environment key used only while the embedded server hands a
/// single-instance trigger to Environment. The handler consumes it before the
/// environment is persisted for the guest.
pub const SINGLE_INSTANCE_LAUNCH_ENV: &str = "RUNTARA_SINGLE_INSTANCE_LAUNCH";

const LAUNCH_COLUMNS: &str = "launch_id, instance_id, tenant_id, image_id, workflow_id, \
    single_instance, kind, state, \
    available_at, deadline_at, lease_owner, lease_expires_at, attempt_count, \
    last_error, created_at, updated_at";
const LAUNCH_RETURNING_COLUMNS: &str = "launch.launch_id, launch.instance_id, \
    launch.tenant_id, launch.image_id, launch.workflow_id, launch.single_instance, \
    launch.kind, launch.state, launch.available_at, \
    launch.deadline_at, launch.lease_owner, launch.lease_expires_at, launch.attempt_count, \
    launch.last_error, launch.created_at, launch.updated_at";

/// The source that requested a runner handoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchKind {
    /// A newly-created instance is starting for the first time.
    Start,
    /// A user explicitly resumed a parked instance.
    Resume,
    /// The durable wake scheduler resumed a due instance.
    Wake,
}

impl LaunchKind {
    /// Database spelling for this launch source.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Resume => "resume",
            Self::Wake => "wake",
        }
    }

    fn from_db(value: String) -> Result<Self, LaunchQueueError> {
        match value.as_str() {
            "start" => Ok(Self::Start),
            "resume" => Ok(Self::Resume),
            "wake" => Ok(Self::Wake),
            _ => Err(LaunchQueueError::UnknownStoredValue {
                field: "kind",
                value,
            }),
        }
    }
}

/// Durable state of one physical launch generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchState {
    /// The launch is durable and eligible for dispatcher claim at `available_at`.
    Queued,
    /// A dispatcher owns bounded artifact validation and component preparation.
    ///
    /// This state deliberately precedes a runner permit. A slow compiler or
    /// artifact read therefore consumes only the independent preparation pool,
    /// never a live guest slot.
    Preparing,
    /// One dispatcher owns a short, recoverable claim.
    Leased,
    /// The dispatcher has acquired capacity and is installing launch ownership.
    Starting,
    /// A runner has been handed the generation and the guest may execute.
    Running,
    /// The guest parked; this generation no longer prevents a future wake.
    Suspended,
    /// The guest completed successfully.
    Completed,
    /// The handoff or guest failed.
    Failed,
    /// Cancellation won before the guest began executing.
    Cancelled,
}

impl LaunchState {
    /// Database spelling for this state.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Preparing => "preparing",
            Self::Leased => "leased",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Suspended => "suspended",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Whether this generation still owns the instance's live launch slot.
    pub const fn is_active(self) -> bool {
        matches!(
            self,
            Self::Queued | Self::Preparing | Self::Leased | Self::Starting | Self::Running
        )
    }

    /// Whether this generation has stopped owning runner/start capacity.
    ///
    /// `Suspended` is terminal for the physical generation even though the
    /// durable instance is not terminal: a later wake creates a new launch.
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Suspended | Self::Completed | Self::Failed | Self::Cancelled
        )
    }

    fn from_db(value: String) -> Result<Self, LaunchQueueError> {
        match value.as_str() {
            "queued" => Ok(Self::Queued),
            "preparing" => Ok(Self::Preparing),
            "leased" => Ok(Self::Leased),
            "starting" => Ok(Self::Starting),
            "running" => Ok(Self::Running),
            "suspended" => Ok(Self::Suspended),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(LaunchQueueError::UnknownStoredValue {
                field: "state",
                value,
            }),
        }
    }
}

/// Workflow identity carried by a launch generation.
///
/// The scope is persisted for ordinary workflow starts too. That preserves the
/// existing workflow-wide behavior: a later `single_instance` trigger sees an
/// active launch created by another source, while parked history remains
/// lease-free.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowLaunchScope {
    /// Workflow whose active generations share this scope.
    pub workflow_id: String,
    /// Whether this logical execution must defer while the scope is active.
    pub single_instance: bool,
}

/// One persisted launch generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    /// Stable idempotency key and generation identifier.
    pub launch_id: String,
    /// Durable Core instance being launched.
    pub instance_id: String,
    /// Tenant that owns the instance and its Environment.
    pub tenant_id: String,
    /// Immutable image selected for this generation.
    pub image_id: String,
    /// Optional durable workflow scope used for `single_instance` admission.
    pub workflow_scope: Option<WorkflowLaunchScope>,
    /// Source of the launch request.
    pub kind: LaunchKind,
    /// Current durable handoff state.
    pub state: LaunchState,
    /// Earliest database time at which a dispatcher may claim it.
    pub available_at: DateTime<Utc>,
    /// Absolute database-time queue deadline.
    pub deadline_at: DateTime<Utc>,
    /// Dispatcher that holds the short claim, if any.
    pub lease_owner: Option<String>,
    /// Time after which a dead dispatcher's claim is recoverable.
    pub lease_expires_at: Option<DateTime<Utc>>,
    /// Number of dispatcher claims made for this generation.
    pub attempt_count: i32,
    /// Most recent queue/launch error, if the generation failed.
    pub last_error: Option<String>,
    /// Time the durable generation was enqueued.
    pub created_at: DateTime<Utc>,
    /// Time this row last changed state or ownership.
    pub updated_at: DateTime<Utc>,
}

/// Data required to enqueue a physical launch.
#[derive(Debug, Clone)]
pub struct EnqueueRequest {
    /// Stable idempotency key for this attempt.
    pub launch_id: String,
    /// Existing durable Core instance to hand to a runner.
    pub instance_id: String,
    /// Tenant that owns the instance.
    pub tenant_id: String,
    /// Existing immutable image selected for the attempt.
    pub image_id: String,
    /// Optional durable workflow scope for the generation.
    pub workflow_scope: Option<WorkflowLaunchScope>,
    /// Source of the request.
    pub kind: LaunchKind,
    /// Delay before the row becomes claimable, measured from database time.
    pub available_after: Duration,
    /// Maximum time the row may wait in the queue, measured from database time.
    pub queue_timeout: Duration,
}

/// Atomic initial-instance claim plus its first durable launch generation.
///
/// A first start must not insert an `instances` row in one transaction and a
/// queue row in another: a crash between those writes recreates the stranded
/// `pending` state this queue replaces.
#[derive(Debug, Clone)]
pub struct InitialLaunchRequest {
    /// The first physical launch generation. Its kind must be [`LaunchKind::Start`].
    pub launch: EnqueueRequest,
    /// Enriched input persisted on the durable Core instance.
    pub input: Option<Vec<u8>>,
    /// Optional custom environment persisted with the image binding.
    pub env: Option<HashMap<String, String>>,
    /// Bounded active-execution timeout persisted with the image binding.
    pub timeout_seconds: Option<i64>,
}

/// Result of atomically claiming an initial instance and launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitialLaunchOutcome {
    /// A new Core instance, image binding, and queued launch were committed together.
    Enqueued(Launch),
    /// Another transaction already owns the instance's live launch generation.
    ExistingLaunch(Launch),
    /// A matching `single_instance` workflow already owns an active launch.
    ///
    /// No Core instance or image binding was committed for this refusal.
    SingleInstanceActive,
    /// The instance ID predates this request but has no live launch generation.
    ///
    /// Callers must inspect the existing durable instance rather than treating
    /// this as a successful start. This keeps an old malformed `pending` row
    /// visible instead of silently reporting it as queued.
    ExistingInstance,
}

impl EnqueueRequest {
    /// Build a launch that is claimable immediately.
    pub fn immediate(
        launch_id: impl Into<String>,
        instance_id: impl Into<String>,
        tenant_id: impl Into<String>,
        image_id: impl Into<String>,
        kind: LaunchKind,
        queue_timeout: Duration,
    ) -> Self {
        Self {
            launch_id: launch_id.into(),
            instance_id: instance_id.into(),
            tenant_id: tenant_id.into(),
            image_id: image_id.into(),
            workflow_scope: None,
            kind,
            available_after: Duration::ZERO,
            queue_timeout,
        }
    }

    /// Attach the workflow-wide scope for this logical execution.
    ///
    /// Empty identifiers are ignored so direct Environment callers without a
    /// workflow identity remain compatible rather than persisting a malformed
    /// scope. A single-instance flag therefore can never exist without a
    /// workflow key.
    pub fn with_workflow_scope(
        mut self,
        workflow_id: impl Into<String>,
        single_instance: bool,
    ) -> Self {
        let workflow_id = workflow_id.into();
        if !workflow_id.is_empty() {
            self.workflow_scope = Some(WorkflowLaunchScope {
                workflow_id,
                single_instance,
            });
        }
        self
    }
}

/// Result of an enqueue request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnqueueOutcome {
    /// This request inserted a new durable launch row.
    Enqueued(Launch),
    /// A row with this launch ID or another active generation already existed.
    Existing(Launch),
    /// A resume/wake inherited `single_instance` scope but another active
    /// generation became the workflow-wide winner first.
    SingleInstanceActive,
}

/// Result of a cancellation attempted before guest execution starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelOutcome {
    /// Cancellation transitioned a queued/leased generation and its instance,
    /// or returned an already-cancelled generation for an idempotent retry.
    Cancelled(Launch),
    /// A generation exists but has passed the cancellable queue states.
    TooLate(Launch),
    /// No launch with this generation ID exists.
    NotFound,
}

/// Failures while reading or transitioning durable launch state.
#[derive(Debug, Error)]
pub enum LaunchQueueError {
    /// PostgreSQL rejected or could not complete a queue operation.
    #[error("launch queue database error: {0}")]
    Database(#[from] sqlx::Error),
    /// A duration cannot be represented by PostgreSQL's microsecond interval.
    #[error("{field} is too large for the launch queue")]
    DurationOutOfRange {
        /// Name of the invalid duration field.
        field: &'static str,
    },
    /// A caller tried to create a lease with no recovery interval.
    #[error("lease duration must be greater than zero")]
    ZeroLeaseDuration,
    /// A database row did not satisfy the queue's closed state machine.
    #[error("unknown stored {field} value {value:?}")]
    UnknownStoredValue {
        /// Column holding an unknown value.
        field: &'static str,
        /// Unexpected stored spelling.
        value: String,
    },
    /// An enqueue conflict vanished before it could be observed or retried.
    #[error("launch enqueue conflicted repeatedly without a durable winner")]
    EnqueueConflictRetryExhausted,
    /// The request does not match the instance's durable image binding or
    /// its permitted pre-launch lifecycle state.
    #[error("launch {launch_id} does not match a launchable instance/image binding")]
    InvalidLaunchTarget {
        /// Generation the caller attempted to enqueue.
        launch_id: String,
    },
    /// An atomic first-launch request used a resume/wake kind.
    #[error("an initial instance claim must use a start launch")]
    InitialLaunchRequiresStart,
    /// A queue transition found an instance outside its matching pre-start state.
    #[error("launch {launch_id} no longer has a cancellable instance")]
    InstanceNoLongerPreStart {
        /// Generation whose paired instance state was inconsistent.
        launch_id: String,
    },
    /// A parking transition found an instance that was no longer running.
    #[error("launch {launch_id} no longer has a running instance")]
    InstanceNoLongerRunning {
        /// Generation whose paired instance state was inconsistent.
        launch_id: String,
    },
    /// A caller tried to bypass the atomic queue/instance parking transition.
    #[error("suspension must use the paired launch/instance parking transaction")]
    SuspensionRequiresParkingTransaction,
    /// A caller asked to terminalize a state that is not terminal.
    #[error("{state:?} is not a terminal launch state")]
    NonTerminalCompletion {
        /// Invalid requested state.
        state: LaunchState,
    },
    /// A stored launch claimed to be single-instance without a workflow key.
    #[error("single-instance launch {launch_id} has no workflow scope")]
    InvalidStoredWorkflowScope {
        /// Durable generation with the malformed scope.
        launch_id: String,
    },
}

/// PostgreSQL repository for [`Launch`] rows.
///
/// This type does not call a runner.  A future dispatcher owns that side
/// effect; keeping this layer transaction-only is what makes lease recovery,
/// expiry, and cancellation safe across process restarts.
#[derive(Clone)]
pub struct LaunchRepository {
    pool: PgPool,
}

impl LaunchRepository {
    /// Create a repository backed by the Environment/Core shared database.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Read one durable generation by its idempotency key.
    pub async fn get(&self, launch_id: &str) -> Result<Option<Launch>, LaunchQueueError> {
        let query = format!("SELECT {LAUNCH_COLUMNS} FROM instance_launches WHERE launch_id = $1");
        sqlx::query_as::<_, LaunchRow>(&query)
            .bind(launch_id)
            .fetch_optional(&self.pool)
            .await?
            .map(Launch::try_from)
            .transpose()
    }

    /// Atomically create a pending Core instance, bind its image, and enqueue
    /// its first generation.
    ///
    /// The transaction is deliberately owned here instead of composing
    /// `claim_instance_with_image` with [`Self::enqueue`]. A process loss
    /// between those two calls would leave an image-bound `pending` instance
    /// with no queue row or launcher, which is exactly the capacity leak this
    /// queue is meant to remove.
    pub async fn claim_initial(
        &self,
        request: InitialLaunchRequest,
    ) -> Result<InitialLaunchOutcome, LaunchQueueError> {
        if request.launch.kind != LaunchKind::Start {
            return Err(LaunchQueueError::InitialLaunchRequiresStart);
        }
        let available_after_us =
            duration_to_micros(request.launch.available_after, "available_after")?;
        let queue_timeout_us = duration_to_micros(request.launch.queue_timeout, "queue_timeout")?;
        let workflow_scope = request.launch.workflow_scope.clone();
        let env = request
            .env
            .filter(|values| !values.is_empty())
            .map(|values| serde_json::to_value(values).unwrap_or_default());
        let active = format!(
            r#"
            SELECT {LAUNCH_COLUMNS}
            FROM instance_launches
            WHERE instance_id = $1
              AND state IN ('queued', 'preparing', 'leased', 'starting', 'running')
            LIMIT 1
            "#
        );
        let insert_launch = format!(
            r#"
            INSERT INTO instance_launches (
                launch_id, instance_id, tenant_id, image_id, workflow_id,
                single_instance, kind, state,
                available_at, deadline_at
            )
            VALUES (
                $1, $2, $3, $4, $5, $6, 'start', 'queued',
                NOW() + ($7 * INTERVAL '1 microsecond'),
                NOW() + ($8 * INTERVAL '1 microsecond')
            )
            RETURNING {LAUNCH_COLUMNS}
            "#
        );

        let mut tx = self.pool.begin().await?;
        let claimed: Option<String> = sqlx::query_scalar(
            r#"
            INSERT INTO instances
                (instance_id, tenant_id, definition_version, status, created_at, input)
            VALUES ($1, $2, 1, 'pending', NOW(), $3)
            ON CONFLICT (instance_id) DO NOTHING
            RETURNING instance_id
            "#,
        )
        .bind(&request.launch.instance_id)
        .bind(&request.launch.tenant_id)
        .bind(request.input.as_deref())
        .fetch_optional(&mut *tx)
        .await?;

        if claimed.is_none() {
            let existing = sqlx::query_as::<_, LaunchRow>(&active)
                .bind(&request.launch.instance_id)
                .fetch_optional(&mut *tx)
                .await?;
            tx.commit().await?;
            return match existing {
                Some(launch) => Ok(InitialLaunchOutcome::ExistingLaunch(launch.try_into()?)),
                None => Ok(InitialLaunchOutcome::ExistingInstance),
            };
        }

        // Every workflow-scoped start takes the same transaction advisory
        // lock, even if this particular source did not opt into
        // single_instance: that is what lets a guarded trigger see regular
        // active workflow work too. Only the guarded start needs it
        // exclusively, so ordinary starts take it shared and stay concurrent.
        if let Some(scope) = workflow_scope.as_ref() {
            acquire_workflow_scope_lock(
                &mut tx,
                &request.launch.tenant_id,
                &scope.workflow_id,
                scope.single_instance,
            )
            .await?;
            if scope.single_instance
                && has_active_workflow_launch(
                    &mut tx,
                    &request.launch.tenant_id,
                    &scope.workflow_id,
                )
                .await?
            {
                // Dropping the transaction rolls back the instance claim,
                // preventing an unowned pending row from surviving a skipped
                // trigger delivery.
                return Ok(InitialLaunchOutcome::SingleInstanceActive);
            }
        }

        // Bind only an image owned by the same tenant. The `RETURNING` result
        // makes a missing or cross-tenant image abort the surrounding instance
        // insert rather than leaving a launchable-looking `pending` row.
        let image_bound: Option<String> = sqlx::query_scalar(
            r#"
            INSERT INTO instance_images
                (instance_id, image_id, tenant_id, env, timeout_seconds, created_at)
            SELECT $1, image_id, $3, $4, $5, NOW()
            FROM images
            WHERE image_id = $2 AND tenant_id = $3
            RETURNING instance_id
            "#,
        )
        .bind(&request.launch.instance_id)
        .bind(&request.launch.image_id)
        .bind(&request.launch.tenant_id)
        .bind(env)
        .bind(request.timeout_seconds)
        .fetch_optional(&mut *tx)
        .await?;
        if image_bound.is_none() {
            return Err(LaunchQueueError::InvalidLaunchTarget {
                launch_id: request.launch.launch_id,
            });
        }

        let launch = sqlx::query_as::<_, LaunchRow>(&insert_launch)
            .bind(&request.launch.launch_id)
            .bind(&request.launch.instance_id)
            .bind(&request.launch.tenant_id)
            .bind(&request.launch.image_id)
            .bind(workflow_scope.as_ref().map(|scope| &scope.workflow_id))
            .bind(
                workflow_scope
                    .as_ref()
                    .is_some_and(|scope| scope.single_instance),
            )
            .bind(available_after_us)
            .bind(queue_timeout_us)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(InitialLaunchOutcome::Enqueued(launch.try_into()?))
    }

    /// Insert a durable row or return its existing idempotent/active winner.
    ///
    /// The insert locks the durable instance long enough to verify its tenant,
    /// image binding, and lifecycle state.  It never makes a queue row for an
    /// image-less or cross-tenant instance.  The partial unique index then
    /// makes distinct starts for one instance converge on one active
    /// generation.  The short retry handles the narrow race where that active
    /// winner became terminal immediately after a conflicting insert was
    /// observed.
    pub async fn enqueue(
        &self,
        request: EnqueueRequest,
    ) -> Result<EnqueueOutcome, LaunchQueueError> {
        let available_after_us = duration_to_micros(request.available_after, "available_after")?;
        let queue_timeout_us = duration_to_micros(request.queue_timeout, "queue_timeout")?;
        let insert = format!(
            r#"
            INSERT INTO instance_launches (
                launch_id, instance_id, tenant_id, image_id, workflow_id,
                single_instance, kind, state,
                available_at, deadline_at
            )
            VALUES (
                $1, $2, $3, $4, $5, $6, $7, 'queued',
                NOW() + ($8 * INTERVAL '1 microsecond'),
                NOW() + ($9 * INTERVAL '1 microsecond')
            )
            ON CONFLICT DO NOTHING
            RETURNING {LAUNCH_COLUMNS}
            "#
        );

        let active = format!(
            r#"
            SELECT {LAUNCH_COLUMNS}
            FROM instance_launches
            WHERE instance_id = $1
              AND state IN ('queued', 'preparing', 'leased', 'starting', 'running')
            LIMIT 1
            "#
        );

        for _ in 0..3 {
            if let Some(existing) = self.get(&request.launch_id).await? {
                return Ok(EnqueueOutcome::Existing(existing));
            }

            let mut tx = self.pool.begin().await?;
            let target: Option<(String, String)> = sqlx::query_as(
                r#"
                SELECT tenant_id, status::TEXT
                FROM instances
                WHERE instance_id = $1
                FOR UPDATE
                "#,
            )
            .bind(&request.instance_id)
            .fetch_optional(&mut *tx)
            .await?;

            let Some((tenant_id, status)) = target else {
                return Err(LaunchQueueError::InvalidLaunchTarget {
                    launch_id: request.launch_id.clone(),
                });
            };

            if let Some(existing) = sqlx::query_as::<_, LaunchRow>(&active)
                .bind(&request.instance_id)
                .fetch_optional(&mut *tx)
                .await?
            {
                tx.commit().await?;
                return Ok(EnqueueOutcome::Existing(existing.try_into()?));
            }

            let expected_status = match request.kind {
                LaunchKind::Start => "pending",
                LaunchKind::Resume | LaunchKind::Wake => "suspended",
            };
            let image_is_bound: bool = sqlx::query_scalar(
                r#"
                SELECT EXISTS(
                    SELECT 1
                    FROM instance_images AS binding
                    JOIN images ON images.image_id = binding.image_id
                    WHERE binding.instance_id = $1
                      AND binding.image_id = $2
                      AND binding.tenant_id = $3
                      AND images.tenant_id = $3
                )
                "#,
            )
            .bind(&request.instance_id)
            .bind(&request.image_id)
            .bind(&request.tenant_id)
            .fetch_one(&mut *tx)
            .await?;
            if tenant_id != request.tenant_id || status != expected_status || !image_is_bound {
                return Err(LaunchQueueError::InvalidLaunchTarget {
                    launch_id: request.launch_id.clone(),
                });
            }

            // Resume and wake generations inherit their scope from the
            // logical instance's earlier launches. A parked instance has no
            // active row/lease, but its next generation must still compete
            // with a concurrently arriving single-instance trigger.
            let historical_scope = sqlx::query_as::<_, WorkflowScopeRow>(
                r#"
                SELECT workflow_id, single_instance
                FROM instance_launches
                WHERE instance_id = $1
                  AND workflow_id IS NOT NULL
                ORDER BY created_at DESC, launch_id DESC
                LIMIT 1
                "#,
            )
            .bind(&request.instance_id)
            .fetch_optional(&mut *tx)
            .await?
            .map(WorkflowLaunchScope::try_from)
            .transpose()?;
            let workflow_scope = match request.kind {
                LaunchKind::Start => request.workflow_scope.clone().or(historical_scope),
                LaunchKind::Resume | LaunchKind::Wake => {
                    historical_scope.or(request.workflow_scope.clone())
                }
            };

            if let Some(scope) = workflow_scope.as_ref() {
                acquire_workflow_scope_lock(
                    &mut tx,
                    &request.tenant_id,
                    &scope.workflow_id,
                    scope.single_instance,
                )
                .await?;
                if scope.single_instance
                    && has_active_workflow_launch(&mut tx, &request.tenant_id, &scope.workflow_id)
                        .await?
                {
                    tx.commit().await?;
                    return Ok(EnqueueOutcome::SingleInstanceActive);
                }
            }

            if let Some(row) = sqlx::query_as::<_, LaunchRow>(&insert)
                .bind(&request.launch_id)
                .bind(&request.instance_id)
                .bind(&request.tenant_id)
                .bind(&request.image_id)
                .bind(workflow_scope.as_ref().map(|scope| &scope.workflow_id))
                .bind(
                    workflow_scope
                        .as_ref()
                        .is_some_and(|scope| scope.single_instance),
                )
                .bind(request.kind.as_str())
                .bind(available_after_us)
                .bind(queue_timeout_us)
                .fetch_optional(&mut *tx)
                .await?
            {
                tx.commit().await?;
                return Ok(EnqueueOutcome::Enqueued(row.try_into()?));
            }

            if let Some(existing) = sqlx::query_as::<_, LaunchRow>(&active)
                .bind(&request.instance_id)
                .fetch_optional(&mut *tx)
                .await?
            {
                tx.commit().await?;
                return Ok(EnqueueOutcome::Existing(existing.try_into()?));
            }

            tx.commit().await?;
            tokio::task::yield_now().await;
        }

        Err(LaunchQueueError::EnqueueConflictRetryExhausted)
    }

    /// Read the one live launch generation for an instance, if it has one.
    ///
    /// This is used by cancellation paths that receive an instance ID rather
    /// than a generation ID. The partial unique index makes the result
    /// unambiguous.
    pub async fn get_active_for_instance(
        &self,
        instance_id: &str,
    ) -> Result<Option<Launch>, LaunchQueueError> {
        let query = format!(
            r#"
            SELECT {LAUNCH_COLUMNS}
            FROM instance_launches
            WHERE instance_id = $1
              AND state IN ('queued', 'preparing', 'leased', 'starting', 'running')
            LIMIT 1
            "#
        );
        sqlx::query_as::<_, LaunchRow>(&query)
            .bind(instance_id)
            .fetch_optional(&self.pool)
            .await?
            .map(Launch::try_from)
            .transpose()
    }

    /// Reconcile active launch rows after their Core instance already parked
    /// or terminalized.
    ///
    /// Normal monitor and cancellation paths transition the Core instance and
    /// launch generation together. If the host dies after Core records the
    /// outcome but before the generation is released, this bounded scan makes
    /// the durable Core state authoritative again. A `suspended` instance is
    /// reconciled only from `running`: a queued/preparing/leased/starting resume or wake
    /// legitimately remains `suspended` until the start gate opens.
    pub async fn reconcile_released_instances(
        &self,
        limit: usize,
    ) -> Result<Vec<Launch>, LaunchQueueError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let query = format!(
            r#"
            WITH candidates AS (
                SELECT launch.launch_id, core_instance.status::TEXT AS instance_status
                FROM instance_launches AS launch
                JOIN instances AS core_instance
                  ON core_instance.instance_id = launch.instance_id
                 AND core_instance.tenant_id = launch.tenant_id
                WHERE launch.state IN ('queued', 'preparing', 'leased', 'starting', 'running')
                  AND (
                        core_instance.status IN ('completed', 'failed', 'cancelled')
                        OR (
                            core_instance.status = 'suspended'
                            AND launch.state = 'running'
                        )
                  )
                ORDER BY launch.updated_at, launch.launch_id
                FOR UPDATE OF launch SKIP LOCKED
                LIMIT $1
            )
            UPDATE instance_launches AS launch
            SET state = CASE candidates.instance_status
                    WHEN 'suspended' THEN 'suspended'
                    WHEN 'completed' THEN 'completed'
                    WHEN 'failed' THEN 'failed'
                    WHEN 'cancelled' THEN 'cancelled'
                END,
                lease_owner = NULL,
                lease_expires_at = NULL,
                start_gate_deadline_at = NULL,
                updated_at = NOW()
            FROM candidates
            WHERE launch.launch_id = candidates.launch_id
            RETURNING {LAUNCH_RETURNING_COLUMNS}
            "#
        );
        rows_to_launches(
            sqlx::query_as::<_, LaunchRow>(&query)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?,
        )
    }

    /// Claim a bounded ready batch for one dispatcher.
    ///
    /// Claims use database time and `FOR UPDATE SKIP LOCKED`, so concurrent
    /// dispatchers never wait on each other and an arbitrary host clock cannot
    /// change which deadline has passed.
    pub async fn claim_ready(
        &self,
        lease_owner: &str,
        lease_for: Duration,
        limit: usize,
    ) -> Result<Vec<Launch>, LaunchQueueError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        if lease_for.is_zero() {
            return Err(LaunchQueueError::ZeroLeaseDuration);
        }
        let lease_us = duration_to_micros(lease_for, "lease_for")?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let query = format!(
            r#"
            WITH claimable AS (
                SELECT launch_id
                FROM instance_launches
                WHERE state = 'queued'
                  AND available_at <= NOW()
                  AND deadline_at > NOW()
                ORDER BY available_at, created_at, launch_id
                FOR UPDATE SKIP LOCKED
                LIMIT $3
            )
            UPDATE instance_launches AS launch
            SET state = 'leased',
                lease_owner = $1,
                lease_expires_at = NOW() + ($2 * INTERVAL '1 microsecond'),
                attempt_count = launch.attempt_count + 1,
                updated_at = NOW()
            FROM claimable
            WHERE launch.launch_id = claimable.launch_id
            RETURNING {LAUNCH_RETURNING_COLUMNS}
            "#
        );
        rows_to_launches(
            sqlx::query_as::<_, LaunchRow>(&query)
                .bind(lease_owner)
                .bind(lease_us)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?,
        )
    }

    /// Claim ready launches for bounded pre-run preparation.
    ///
    /// A preparation claim never holds a guest/run permit. It owns only a
    /// short durable lease and the runner's independently bounded preparation
    /// slot, so compiling a malformed or slow component cannot fill the live
    /// execution pool. Like ordinary claims, this is database-time based and
    /// uses `SKIP LOCKED` across dispatcher processes.
    pub async fn claim_ready_for_preparation(
        &self,
        lease_owner: &str,
        lease_for: Duration,
        limit: usize,
    ) -> Result<Vec<Launch>, LaunchQueueError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        if lease_for.is_zero() {
            return Err(LaunchQueueError::ZeroLeaseDuration);
        }
        let lease_us = duration_to_micros(lease_for, "preparation_lease_for")?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let query = format!(
            r#"
            WITH claimable AS (
                SELECT launch_id
                FROM instance_launches
                WHERE state = 'queued'
                  AND available_at <= NOW()
                  AND deadline_at > NOW()
                ORDER BY available_at, created_at, launch_id
                FOR UPDATE SKIP LOCKED
                LIMIT $3
            )
            UPDATE instance_launches AS launch
            SET state = 'preparing',
                lease_owner = $1,
                lease_expires_at = NOW() + ($2 * INTERVAL '1 microsecond'),
                attempt_count = launch.attempt_count + 1,
                updated_at = NOW()
            FROM claimable
            WHERE launch.launch_id = claimable.launch_id
            RETURNING {LAUNCH_RETURNING_COLUMNS}
            "#
        );
        rows_to_launches(
            sqlx::query_as::<_, LaunchRow>(&query)
                .bind(lease_owner)
                .bind(lease_us)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?,
        )
    }

    /// Return expired preparation claims to the ready queue.
    ///
    /// The background compile/read task may still be unwinding when this
    /// transition commits. Its killable child is reaped outside the parent
    /// preparation token, and a late task cannot promote because
    /// [`Self::promote_prepared`] fences on the claim incarnation as well as
    /// this owner and lease.
    pub async fn recover_expired_preparations(
        &self,
        retry_after: Duration,
        limit: usize,
    ) -> Result<Vec<Launch>, LaunchQueueError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let retry_after_us = duration_to_micros(retry_after, "preparation_retry_after")?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut tx = self.pool.begin().await?;
        let select = format!(
            r#"
            SELECT {LAUNCH_COLUMNS}
            FROM instance_launches
            WHERE state = 'preparing'
              AND lease_expires_at <= clock_timestamp()
              AND deadline_at > clock_timestamp()
            ORDER BY lease_expires_at, created_at, launch_id
            FOR UPDATE SKIP LOCKED
            LIMIT $1
            "#
        );
        let expired = sqlx::query_as::<_, LaunchRow>(&select)
            .bind(limit)
            .fetch_all(&mut *tx)
            .await?;
        if expired.is_empty() {
            tx.commit().await?;
            return Ok(Vec::new());
        }

        let ids: Vec<String> = expired.iter().map(|row| row.launch_id.clone()).collect();
        let update = format!(
            r#"
            UPDATE instance_launches
            SET state = 'queued',
                available_at = NOW() + ($2 * INTERVAL '1 microsecond'),
                lease_owner = NULL,
                lease_expires_at = NULL,
                start_gate_deadline_at = NULL,
                last_error = $3,
                updated_at = NOW()
            WHERE launch_id = ANY($1)
              AND state = 'preparing'
            RETURNING {LAUNCH_COLUMNS}
            "#
        );
        let recovered = sqlx::query_as::<_, LaunchRow>(&update)
            .bind(&ids)
            .bind(retry_after_us)
            .bind(PREPARATION_TIMEOUT)
            .fetch_all(&mut *tx)
            .await?;
        tx.commit().await?;
        rows_to_launches(recovered)
    }

    /// Promote a completed preparation back to a short handoff lease.
    ///
    /// The caller still owns the `preparing` lease while it has an opaque
    /// prepared artifact. This one conditional update proves cancellation,
    /// expiry, or another dispatcher's recovery did not win before the
    /// runner can be asked for a guest permit. `preparation_attempt` is the
    /// claim incarnation returned by [`Self::claim_ready_for_preparation`];
    /// it prevents a late compiler from promoting a newer same-owner claim.
    pub async fn promote_prepared(
        &self,
        launch_id: &str,
        lease_owner: &str,
        preparation_attempt: i32,
        handoff_lease_for: Duration,
    ) -> Result<Option<Launch>, LaunchQueueError> {
        if handoff_lease_for.is_zero() {
            return Err(LaunchQueueError::ZeroLeaseDuration);
        }
        let handoff_lease_us = duration_to_micros(handoff_lease_for, "handoff_lease_for")?;
        let query = format!(
            r#"
            UPDATE instance_launches
            SET state = 'leased',
                lease_expires_at = NOW() + ($4 * INTERVAL '1 microsecond'),
                last_error = NULL,
                updated_at = NOW()
            WHERE launch_id = $1
              AND state = 'preparing'
              AND lease_owner = $2
              AND attempt_count = $3
              AND lease_expires_at > clock_timestamp()
              AND deadline_at > clock_timestamp()
            RETURNING {LAUNCH_COLUMNS}
            "#
        );
        sqlx::query_as::<_, LaunchRow>(&query)
            .bind(launch_id)
            .bind(lease_owner)
            .bind(preparation_attempt)
            .bind(handoff_lease_us)
            .fetch_optional(&self.pool)
            .await?
            .map(Launch::try_from)
            .transpose()
    }

    /// Requeue bounded handoffs whose dispatcher died before it opened the
    /// generation's start gate.
    ///
    /// `starting` is now recoverable as well as `leased`, but only when the
    /// durable start-gate marker says this version created it. That rollout
    /// fence prevents a new binary from reclaiming an older version's
    /// `starting` row, whose guest may already be executing.
    pub async fn recover_expired_leases(
        &self,
        limit: usize,
    ) -> Result<Vec<Launch>, LaunchQueueError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut tx = self.pool.begin().await?;
        let select = format!(
            r#"
            SELECT {LAUNCH_COLUMNS}
            FROM instance_launches
            WHERE (
                    state = 'leased'
                    OR (state = 'starting' AND start_gate_deadline_at IS NOT NULL)
                )
              AND lease_expires_at <= clock_timestamp()
              AND deadline_at > clock_timestamp()
            ORDER BY lease_expires_at, created_at, launch_id
            FOR UPDATE SKIP LOCKED
            LIMIT $1
            "#
        );
        let leased = sqlx::query_as::<_, LaunchRow>(&select)
            .bind(limit)
            .fetch_all(&mut *tx)
            .await?;
        if leased.is_empty() {
            tx.commit().await?;
            return Ok(Vec::new());
        }

        let ids: Vec<String> = leased.iter().map(|row| row.launch_id.clone()).collect();
        let update = format!(
            r#"
            UPDATE instance_launches
            SET state = 'queued',
                available_at = NOW(),
                lease_owner = NULL,
                lease_expires_at = NULL,
                start_gate_deadline_at = NULL,
                updated_at = NOW()
            WHERE launch_id = ANY($1)
              AND (
                    state = 'leased'
                    OR (state = 'starting' AND start_gate_deadline_at IS NOT NULL)
                )
            RETURNING {LAUNCH_COLUMNS}
            "#
        );
        let recovered = sqlx::query_as::<_, LaunchRow>(&update)
            .bind(&ids)
            .fetch_all(&mut *tx)
            .await?;
        tx.commit().await?;
        rows_to_launches(recovered)
    }

    /// Move one still-owned lease into the runner-installation phase.
    ///
    /// A dispatcher calls this after nonblocking capacity acquisition.  A
    /// cancellation that won while it was acquiring capacity changes the row
    /// first, so this conditional update returns `None` and the dispatcher
    /// must release the permit without starting a guest. The claim incarnation
    /// is also required so a stale same-owner dispatcher cannot advance a
    /// re-claimed generation.
    pub async fn begin_start(
        &self,
        launch_id: &str,
        lease_owner: &str,
        attempt_count: i32,
    ) -> Result<Option<Launch>, LaunchQueueError> {
        let query = format!(
            r#"
            UPDATE instance_launches
            SET state = 'starting',
                -- This durable marker is the rollout fence for recovery:
                -- only a start that was created behind a gate can be safely
                -- reclaimed or terminalized while it remains unconfirmed.
                start_gate_deadline_at = lease_expires_at,
                updated_at = NOW()
            WHERE launch_id = $1
              AND state = 'leased'
              AND lease_owner = $2
              AND attempt_count = $3
              AND lease_expires_at > clock_timestamp()
              AND deadline_at > clock_timestamp()
            RETURNING {LAUNCH_COLUMNS}
            "#
        );
        sqlx::query_as::<_, LaunchRow>(&query)
            .bind(launch_id)
            .bind(lease_owner)
            .bind(attempt_count)
            .fetch_optional(&self.pool)
            .await?
            .map(Launch::try_from)
            .transpose()
    }

    /// Atomically promote a generation and its Core instance through the
    /// closed start gate.
    ///
    /// The dispatcher calls this only after the runner has accepted a closed
    /// gate and after it has durably registered that generation. Updating the
    /// queue row and Core lifecycle row in one transaction means no guest can
    /// observe execution while either side still says `pending`/`suspended`.
    pub async fn mark_running(
        &self,
        launch_id: &str,
        lease_owner: &str,
        attempt_count: i32,
    ) -> Result<Option<Launch>, LaunchQueueError> {
        let mut tx = self.pool.begin().await?;
        let query = format!(
            r#"
            UPDATE instance_launches
            SET state = 'running',
                lease_owner = NULL,
                lease_expires_at = NULL,
                updated_at = NOW()
            WHERE launch_id = $1
              AND state = 'starting'
              AND lease_owner = $2
              AND attempt_count = $3
              AND lease_expires_at > clock_timestamp()
              AND deadline_at > clock_timestamp()
            RETURNING {LAUNCH_COLUMNS}
            "#
        );
        let running = sqlx::query_as::<_, LaunchRow>(&query)
            .bind(launch_id)
            .bind(lease_owner)
            .bind(attempt_count)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(running) = running else {
            tx.commit().await?;
            return Ok(None);
        };

        let expected_status = match running.kind.as_str() {
            "start" => "pending",
            "resume" | "wake" => "suspended",
            // `LaunchRow` is decoded by `LaunchKind` below, but preserve a
            // closed error path if the database ever contains a new spelling.
            _ => {
                return Err(LaunchQueueError::UnknownStoredValue {
                    field: "kind",
                    value: running.kind,
                });
            }
        };
        let promoted = sqlx::query(
            r#"
            UPDATE instances
            SET status = 'running',
                started_at = COALESCE(started_at, NOW()),
                finished_at = NULL,
                sleep_until = NULL,
                termination_reason = NULL
            WHERE instance_id = $1
              AND tenant_id = $2
              AND status = $3::instance_status
            "#,
        )
        .bind(&running.instance_id)
        .bind(&running.tenant_id)
        .bind(expected_status)
        .execute(&mut *tx)
        .await?;
        if promoted.rows_affected() != 1 {
            return Err(LaunchQueueError::InstanceNoLongerPreStart {
                launch_id: running.launch_id,
            });
        }

        tx.commit().await?;
        Ok(Some(running.try_into()?))
    }

    /// Confirm that a `running` generation may open its in-memory start gate.
    ///
    /// The generation remains marked with its durable gate deadline after
    /// [`Self::mark_running`] so a process loss before guest work begins is
    /// still distinguishable from normal execution. This conditional update
    /// removes that recovery marker only while the same running incarnation is
    /// still within its gate deadline; the runner calls it at the component
    /// host's immediate pre-instantiation boundary.
    pub async fn confirm_gate_open(
        &self,
        launch_id: &str,
        attempt_count: i32,
    ) -> Result<Option<Launch>, LaunchQueueError> {
        let query = format!(
            r#"
            -- Materialize the row lock before reading a real timestamp.
            -- A plain UPDATE may evaluate its WHERE predicate while scanning,
            -- then wait on a concurrent row lock; that would preserve an
            -- already-expired timestamp value despite using the volatile
            -- clock. The second materialized CTE evaluates the timestamp only
            -- after the lock CTE completes, so the outer UPDATE cannot inspect
            -- either deadline until the row lock has been acquired.
            WITH locked(locked_launch_id) AS MATERIALIZED (
                SELECT locked_source.launch_id
                FROM instance_launches AS locked_source
                WHERE locked_source.launch_id = $1
                FOR UPDATE
            ), checked(locked_launch_id, checked_at) AS MATERIALIZED (
                SELECT locked.locked_launch_id, clock_timestamp()
                FROM locked
            )
            UPDATE instance_launches AS launch
            SET start_gate_deadline_at = NULL,
                updated_at = NOW()
            FROM checked
            WHERE launch.launch_id = checked.locked_launch_id
              AND launch.state = 'running'
              AND launch.attempt_count = $2
              AND launch.start_gate_deadline_at > checked.checked_at
              AND launch.deadline_at > checked.checked_at
            RETURNING {LAUNCH_COLUMNS}
            "#
        );
        sqlx::query_as::<_, LaunchRow>(&query)
            .bind(launch_id)
            .bind(attempt_count)
            .fetch_optional(&self.pool)
            .await?
            .map(Launch::try_from)
            .transpose()
    }

    /// Read back an ambiguous start-gate confirmation by its exact durable
    /// incarnation.
    ///
    /// PostgreSQL can commit an `UPDATE` even when the caller loses the
    /// response.  The runner must distinguish that harmless transport failure
    /// from a confirmation that did not commit: only the former may enter
    /// guest code with a cleared marker.
    pub async fn is_gate_confirmed(
        &self,
        launch_id: &str,
        attempt_count: i32,
    ) -> Result<bool, LaunchQueueError> {
        let confirmed: Option<bool> = sqlx::query_scalar(
            r#"
            SELECT state = 'running'
               AND attempt_count = $2
               AND start_gate_deadline_at IS NULL
            FROM instance_launches
            WHERE launch_id = $1
            "#,
        )
        .bind(launch_id)
        .bind(attempt_count)
        .fetch_optional(&self.pool)
        .await?;
        Ok(confirmed.unwrap_or(false))
    }

    /// Fail a `running` generation which never crossed its durable start
    /// gate.
    ///
    /// This is deliberately fenced by `attempt_count`: a previous monitor
    /// must never terminalize an instance after lease recovery gave the same
    /// launch id to a new runner attempt. The queue and Core updates commit
    /// together, so the admission release observer runs only after both are
    /// durable.
    pub async fn fail_unconfirmed_running(
        &self,
        launch_id: &str,
        attempt_count: i32,
        error: &str,
    ) -> Result<Option<Launch>, LaunchQueueError> {
        let mut tx = self.pool.begin().await?;
        let query = format!(
            r#"
            UPDATE instance_launches
            SET state = 'failed',
                lease_owner = NULL,
                lease_expires_at = NULL,
                start_gate_deadline_at = NULL,
                last_error = $3,
                updated_at = NOW()
            WHERE launch_id = $1
              AND state = 'running'
              AND attempt_count = $2
              AND start_gate_deadline_at IS NOT NULL
            RETURNING {LAUNCH_COLUMNS}
            "#
        );
        let failed = sqlx::query_as::<_, LaunchRow>(&query)
            .bind(launch_id)
            .bind(attempt_count)
            .bind(error)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(failed) = failed else {
            tx.commit().await?;
            return Ok(None);
        };

        let updated = sqlx::query(
            r#"
            UPDATE instances
            SET status = 'failed',
                finished_at = NOW(),
                sleep_until = NULL,
                termination_reason = 'start_gate_failed',
                error = $2
            WHERE instance_id = $1
              AND status = 'running'
            "#,
        )
        .bind(&failed.instance_id)
        .bind(error)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(LaunchQueueError::InstanceNoLongerRunning {
                launch_id: failed.launch_id,
            });
        }

        tx.commit().await?;
        Ok(Some(failed.try_into()?))
    }

    /// Return a dispatcher-owned pre-run launch to the ready queue.
    ///
    /// This is deliberately narrower than a generic retry transition: it only
    /// accepts a still-owned lease or start handoff with the exact claimed
    /// incarnation. In particular, it cannot resurrect a generation after
    /// cancellation, recovery by the same dispatcher owner, or a terminal
    /// monitor result has won the race.
    pub async fn requeue_owned(
        &self,
        launch_id: &str,
        lease_owner: &str,
        attempt_count: i32,
        retry_after: Duration,
        last_error: Option<&str>,
    ) -> Result<Option<Launch>, LaunchQueueError> {
        let retry_after_us = duration_to_micros(retry_after, "retry_after")?;
        let query = format!(
            r#"
            UPDATE instance_launches
            SET state = 'queued',
                available_at = NOW() + ($4 * INTERVAL '1 microsecond'),
                lease_owner = NULL,
                lease_expires_at = NULL,
                start_gate_deadline_at = NULL,
                last_error = $5,
                updated_at = NOW()
            WHERE launch_id = $1
              AND lease_owner = $2
              AND attempt_count = $3
              AND state IN ('preparing', 'leased', 'starting')
              AND lease_expires_at > clock_timestamp()
              AND deadline_at > clock_timestamp()
            RETURNING {LAUNCH_COLUMNS}
            "#
        );
        sqlx::query_as::<_, LaunchRow>(&query)
            .bind(launch_id)
            .bind(lease_owner)
            .bind(attempt_count)
            .bind(retry_after_us)
            .bind(last_error)
            .fetch_optional(&self.pool)
            .await?
            .map(Launch::try_from)
            .transpose()
    }

    /// Fail a dispatcher-owned launch before a runner has accepted it.
    ///
    /// The queue and Core updates commit together. This is the counterpart to
    /// [`Self::cancel_before_start`]: a missing artifact, unsupported ABI, or
    /// other pre-run error must not leave an admission-consuming `pending` or
    /// `suspended` instance behind once its launch has been terminalized.
    pub async fn fail_before_runner(
        &self,
        launch_id: &str,
        lease_owner: &str,
        attempt_count: i32,
        error: &str,
    ) -> Result<Option<Launch>, LaunchQueueError> {
        let mut tx = self.pool.begin().await?;
        let update = format!(
            r#"
            UPDATE instance_launches
            SET state = 'failed',
                lease_owner = NULL,
                lease_expires_at = NULL,
                start_gate_deadline_at = NULL,
                last_error = $4,
                updated_at = NOW()
            WHERE launch_id = $1
              AND lease_owner = $2
              AND attempt_count = $3
              AND state IN ('preparing', 'leased', 'starting')
              AND lease_expires_at > clock_timestamp()
              AND deadline_at > clock_timestamp()
            RETURNING {LAUNCH_COLUMNS}
            "#
        );
        let failed = sqlx::query_as::<_, LaunchRow>(&update)
            .bind(launch_id)
            .bind(lease_owner)
            .bind(attempt_count)
            .bind(error)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(failed) = failed else {
            tx.commit().await?;
            return Ok(None);
        };

        let updated = sqlx::query(
            r#"
            UPDATE instances
            SET status = 'failed',
                finished_at = NOW(),
                sleep_until = NULL,
                termination_reason = 'crashed',
                error = $2
            WHERE instance_id = $1
              AND status IN ('pending', 'suspended')
            "#,
        )
        .bind(&failed.instance_id)
        .bind(error)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(LaunchQueueError::InstanceNoLongerPreStart {
                launch_id: failed.launch_id,
            });
        }

        tx.commit().await?;
        Ok(Some(failed.try_into()?))
    }

    /// Atomically release a running generation after its durable instance parks.
    ///
    /// This is intentionally terminal for the launch but not the instance,
    /// allowing a subsequent manual resume or wake to enqueue a new generation.
    pub async fn mark_suspended(
        &self,
        launch_id: &str,
    ) -> Result<Option<Launch>, LaunchQueueError> {
        let mut tx = self.pool.begin().await?;
        let query = format!(
            r#"
            UPDATE instance_launches
            SET state = 'suspended',
                lease_owner = NULL,
                lease_expires_at = NULL,
                start_gate_deadline_at = NULL,
                updated_at = NOW()
            WHERE launch_id = $1
              AND state IN ('starting', 'running')
            RETURNING {LAUNCH_COLUMNS}
            "#
        );
        let suspended = sqlx::query_as::<_, LaunchRow>(&query)
            .bind(launch_id)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(suspended) = suspended else {
            tx.commit().await?;
            return Ok(None);
        };

        let updated = sqlx::query(
            r#"
            UPDATE instances
            SET status = 'suspended',
                finished_at = NULL
            WHERE instance_id = $1
              AND status IN ('running', 'suspended')
            "#,
        )
        .bind(&suspended.instance_id)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(LaunchQueueError::InstanceNoLongerRunning {
                launch_id: suspended.launch_id,
            });
        }

        tx.commit().await?;
        Ok(Some(suspended.try_into()?))
    }

    /// Mark an active generation terminal after runner ownership has stopped.
    pub async fn mark_terminal(
        &self,
        launch_id: &str,
        state: LaunchState,
        last_error: Option<&str>,
    ) -> Result<Option<Launch>, LaunchQueueError> {
        if state == LaunchState::Suspended {
            return Err(LaunchQueueError::SuspensionRequiresParkingTransaction);
        }
        if !state.is_terminal() {
            return Err(LaunchQueueError::NonTerminalCompletion { state });
        }
        let query = format!(
            r#"
            UPDATE instance_launches
            SET state = $2,
                lease_owner = NULL,
                lease_expires_at = NULL,
                start_gate_deadline_at = NULL,
                last_error = $3,
                updated_at = NOW()
            WHERE launch_id = $1
              AND state IN ('queued', 'preparing', 'leased', 'starting', 'running')
            RETURNING {LAUNCH_COLUMNS}
            "#
        );
        sqlx::query_as::<_, LaunchRow>(&query)
            .bind(launch_id)
            .bind(state.as_str())
            .bind(last_error)
            .fetch_optional(&self.pool)
            .await?
            .map(Launch::try_from)
            .transpose()
    }

    /// Fail a bounded batch whose queue deadline or unconfirmed start-gate
    /// deadline has elapsed.
    ///
    /// The matching Core instances are terminalized in the same transaction,
    /// immediately releasing their existing pending admission occupancy.  The
    /// later durable-admission layer may add its own reservation release to
    /// this transaction without changing the launch race semantics.
    pub async fn expire_due(&self, limit: usize) -> Result<Vec<Launch>, LaunchQueueError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut tx = self.pool.begin().await?;
        let select = format!(
            r#"
            SELECT {LAUNCH_COLUMNS}
            FROM instance_launches
            WHERE (
                    state IN ('queued', 'preparing', 'leased')
                    OR (state = 'starting' AND start_gate_deadline_at IS NOT NULL)
                )
              AND deadline_at <= clock_timestamp()
              OR (
                    state = 'running'
                    AND start_gate_deadline_at IS NOT NULL
                    AND (
                        start_gate_deadline_at <= clock_timestamp()
                        OR deadline_at <= clock_timestamp()
                    )
                )
            ORDER BY deadline_at, created_at, launch_id
            FOR UPDATE SKIP LOCKED
            LIMIT $1
            "#
        );
        let due = sqlx::query_as::<_, LaunchRow>(&select)
            .bind(limit)
            .fetch_all(&mut *tx)
            .await?;
        if due.is_empty() {
            tx.commit().await?;
            return Ok(Vec::new());
        }

        let launch_ids: Vec<String> = due.iter().map(|row| row.launch_id.clone()).collect();
        let instance_ids: Vec<String> = due.iter().map(|row| row.instance_id.clone()).collect();
        let update_launches = format!(
            r#"
            UPDATE instance_launches
            SET state = 'failed',
                lease_owner = NULL,
                lease_expires_at = NULL,
                start_gate_deadline_at = NULL,
                last_error = $2,
                updated_at = NOW()
            WHERE launch_id = ANY($1)
              AND (
                    (
                        state IN ('queued', 'preparing', 'leased')
                        OR (state = 'starting' AND start_gate_deadline_at IS NOT NULL)
                    )
                    AND deadline_at <= clock_timestamp()
                    OR (
                        state = 'running'
                        AND start_gate_deadline_at IS NOT NULL
                        AND (
                            start_gate_deadline_at <= clock_timestamp()
                            OR deadline_at <= clock_timestamp()
                        )
                    )
                )
            RETURNING {LAUNCH_COLUMNS}
            "#
        );
        let expired = sqlx::query_as::<_, LaunchRow>(&update_launches)
            .bind(&launch_ids)
            .bind(LAUNCH_QUEUE_TIMEOUT)
            .fetch_all(&mut *tx)
            .await?;
        if expired.len() != due.len() {
            return Err(LaunchQueueError::InstanceNoLongerPreStart {
                launch_id: due[0].launch_id.clone(),
            });
        }

        let terminalized: Vec<String> = sqlx::query_scalar(
            r#"
            UPDATE instances
            SET status = 'failed',
                finished_at = NOW(),
                sleep_until = NULL,
                termination_reason = 'launch_queue_timeout',
                error = $2
            WHERE instance_id = ANY($1)
              AND status IN ('pending', 'suspended', 'running')
            RETURNING instance_id
            "#,
        )
        .bind(&instance_ids)
        .bind(LAUNCH_QUEUE_TIMEOUT)
        .fetch_all(&mut *tx)
        .await?;
        if terminalized.len() != instance_ids.len() {
            return Err(LaunchQueueError::InstanceNoLongerPreStart {
                launch_id: due[0].launch_id.clone(),
            });
        }

        tx.commit().await?;
        rows_to_launches(expired)
    }

    /// Cancel a queued, leased, or still-unconfirmed gate generation and
    /// terminalize its Core instance.
    ///
    /// This conditional transaction is the pre-start cancellation fence: if a
    /// dispatcher has already opened the start gate (`running`), cancellation
    /// is deliberately reported as [`CancelOutcome::TooLate`] so normal
    /// generation-specific runner cancellation owns the outcome instead.
    pub async fn cancel_before_start(
        &self,
        launch_id: &str,
    ) -> Result<CancelOutcome, LaunchQueueError> {
        let mut tx = self.pool.begin().await?;
        let update = format!(
            r#"
            UPDATE instance_launches
            SET state = 'cancelled',
                lease_owner = NULL,
                lease_expires_at = NULL,
                start_gate_deadline_at = NULL,
                updated_at = NOW()
            WHERE launch_id = $1
              AND (
                    state IN ('queued', 'preparing', 'leased')
                    OR (state IN ('starting', 'running') AND start_gate_deadline_at IS NOT NULL)
                )
            RETURNING {LAUNCH_COLUMNS}
            "#
        );
        let cancelled = sqlx::query_as::<_, LaunchRow>(&update)
            .bind(launch_id)
            .fetch_optional(&mut *tx)
            .await?;

        if let Some(cancelled) = cancelled {
            let updated = sqlx::query(
                r#"
                UPDATE instances
                SET status = 'cancelled',
                    finished_at = NOW(),
                    sleep_until = NULL,
                    termination_reason = 'cancelled'
                WHERE instance_id = $1
                  AND status IN ('pending', 'suspended', 'running')
                "#,
            )
            .bind(&cancelled.instance_id)
            .execute(&mut *tx)
            .await?;
            if updated.rows_affected() != 1 {
                return Err(LaunchQueueError::InstanceNoLongerPreStart {
                    launch_id: cancelled.launch_id,
                });
            }
            tx.commit().await?;
            return Ok(CancelOutcome::Cancelled(cancelled.try_into()?));
        }

        let query = format!("SELECT {LAUNCH_COLUMNS} FROM instance_launches WHERE launch_id = $1");
        let existing = sqlx::query_as::<_, LaunchRow>(&query)
            .bind(launch_id)
            .fetch_optional(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(match existing {
            Some(row) => {
                let launch: Launch = row.try_into()?;
                if launch.state == LaunchState::Cancelled {
                    CancelOutcome::Cancelled(launch)
                } else {
                    CancelOutcome::TooLate(launch)
                }
            }
            None => CancelOutcome::NotFound,
        })
    }
}

#[derive(Debug, FromRow)]
struct LaunchRow {
    launch_id: String,
    instance_id: String,
    tenant_id: String,
    image_id: String,
    workflow_id: Option<String>,
    single_instance: bool,
    kind: String,
    state: String,
    available_at: DateTime<Utc>,
    deadline_at: DateTime<Utc>,
    lease_owner: Option<String>,
    lease_expires_at: Option<DateTime<Utc>>,
    attempt_count: i32,
    last_error: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct WorkflowScopeRow {
    workflow_id: String,
    single_instance: bool,
}

impl TryFrom<WorkflowScopeRow> for WorkflowLaunchScope {
    type Error = LaunchQueueError;

    fn try_from(row: WorkflowScopeRow) -> Result<Self, Self::Error> {
        Ok(Self {
            workflow_id: row.workflow_id,
            single_instance: row.single_instance,
        })
    }
}

impl TryFrom<LaunchRow> for Launch {
    type Error = LaunchQueueError;

    fn try_from(row: LaunchRow) -> Result<Self, Self::Error> {
        let workflow_scope = match row.workflow_id {
            Some(workflow_id) => Some(WorkflowLaunchScope {
                workflow_id,
                single_instance: row.single_instance,
            }),
            None if row.single_instance => {
                return Err(LaunchQueueError::InvalidStoredWorkflowScope {
                    launch_id: row.launch_id,
                });
            }
            None => None,
        };
        Ok(Self {
            launch_id: row.launch_id,
            instance_id: row.instance_id,
            tenant_id: row.tenant_id,
            image_id: row.image_id,
            workflow_scope,
            kind: LaunchKind::from_db(row.kind)?,
            state: LaunchState::from_db(row.state)?,
            available_at: row.available_at,
            deadline_at: row.deadline_at,
            lease_owner: row.lease_owner,
            lease_expires_at: row.lease_expires_at,
            attempt_count: row.attempt_count,
            last_error: row.last_error,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

/// Take the workflow-scope launch admission lock on one PostgreSQL
/// transaction advisory lock, in the weakest mode that still holds the
/// single-instance invariant.
///
/// The durable active-row query below is the actual lease. The advisory lock
/// only makes its check-plus-insert atomic across server processes and is
/// released automatically with this transaction on commit, rollback, or a
/// process crash.
///
/// Mode matters for throughput. A `single_instance` start is the only caller
/// that reads the workflow's active set, so it takes the lock exclusively: it
/// then waits out every in-flight ordinary launch and blocks new ones, and so
/// observes their rows committed before deciding. Ordinary launches only need
/// to be *visible* to that check, never to exclude one another, so they take
/// the shared mode and proceed concurrently. Holding the exclusive mode for
/// every launch serialized a hot workflow's entire pipeline on one lock.
async fn acquire_workflow_scope_lock(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    workflow_id: &str,
    single_instance: bool,
) -> Result<(), LaunchQueueError> {
    let scope_key = format!("{tenant_id}\u{1f}{workflow_id}");
    let sql = if single_instance {
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))"
    } else {
        "SELECT pg_advisory_xact_lock_shared(hashtextextended($1, 0))"
    };
    sqlx::query(sql).bind(scope_key).execute(&mut **tx).await?;
    Ok(())
}

/// Whether any active generation for a workflow currently owns its durable
/// lease. The caller must hold [`acquire_workflow_scope_lock`] for this key.
async fn has_active_workflow_launch(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    workflow_id: &str,
) -> Result<bool, LaunchQueueError> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM instance_launches
            WHERE tenant_id = $1
              AND workflow_id = $2
              AND state IN ('queued', 'preparing', 'leased', 'starting', 'running')
        )
        "#,
    )
    .bind(tenant_id)
    .bind(workflow_id)
    .fetch_one(&mut **tx)
    .await?)
}

fn rows_to_launches(rows: Vec<LaunchRow>) -> Result<Vec<Launch>, LaunchQueueError> {
    rows.into_iter().map(Launch::try_from).collect()
}

fn duration_to_micros(duration: Duration, field: &'static str) -> Result<i64, LaunchQueueError> {
    i64::try_from(duration.as_micros()).map_err(|_| LaunchQueueError::DurationOutOfRange { field })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_live_handoffs_hold_the_per_instance_slot() {
        for state in [
            LaunchState::Queued,
            LaunchState::Leased,
            LaunchState::Starting,
            LaunchState::Running,
        ] {
            assert!(state.is_active(), "{state:?} must retain the launch slot");
            assert!(!state.is_terminal());
        }
        for state in [
            LaunchState::Suspended,
            LaunchState::Completed,
            LaunchState::Failed,
            LaunchState::Cancelled,
        ] {
            assert!(state.is_terminal(), "{state:?} releases the launch slot");
            assert!(!state.is_active());
        }
    }

    #[test]
    fn duration_conversion_refuses_an_unrepresentable_database_interval() {
        let result = duration_to_micros(Duration::from_secs(u64::MAX), "queue_timeout");
        assert!(matches!(
            result,
            Err(LaunchQueueError::DurationOutOfRange {
                field: "queue_timeout"
            })
        ));
    }

    #[test]
    fn immediate_requests_are_ready_without_a_host_clock_timestamp() {
        let request = EnqueueRequest::immediate(
            "launch-1",
            "instance-1",
            "tenant-1",
            "image-1",
            LaunchKind::Start,
            Duration::from_secs(60),
        );
        assert_eq!(request.available_after, Duration::ZERO);
        assert_eq!(request.queue_timeout, Duration::from_secs(60));
    }

    #[test]
    fn database_spellings_match_the_closed_state_machine() {
        assert_eq!(LaunchState::Queued.as_str(), "queued");
        assert_eq!(LaunchState::Leased.as_str(), "leased");
        assert_eq!(LaunchState::Starting.as_str(), "starting");
        assert_eq!(LaunchState::Running.as_str(), "running");
        assert_eq!(LaunchState::Suspended.as_str(), "suspended");
        assert_eq!(LaunchState::Completed.as_str(), "completed");
        assert_eq!(LaunchState::Failed.as_str(), "failed");
        assert_eq!(LaunchState::Cancelled.as_str(), "cancelled");
    }
}
