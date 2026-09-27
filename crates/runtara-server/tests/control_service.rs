//! The native control service over real runtime persistence: tenant-wide
//! reads, paging, inline caps and error codes (decision D5: every tier).
use std::sync::Arc;
use std::time::Duration;

use runtara_component_host::control_host::{
    ControlAuthority, ControlErrorCode, ControlHost, InstanceStatus, ParentFilter,
    PendingSignalsRequest, QueryRequest, SignalScope, SortField, SortOrder, StartRequest,
    SuspensionReason,
};
use runtara_core::{
    domain::InstanceStatus as Core,
    persistence::{
        CompleteInstanceParams, Persistence,
        inputs::{InputAuthority, InputRequestSpec},
    },
};
use runtara_environment::{handlers::EnvironmentHandlerState, runner::MockRunner};
use runtara_server::api::services::control::NativeControl;
use runtara_server::runtime_client::{RuntimeClient, RuntimeClientConfig};
use runtara_store_postgres::PostgresPersistence;
use serde_json::{Value, json};
use uuid::Uuid;

struct Fixture {
    pool: sqlx::PgPool,
    persistence: Arc<PostgresPersistence>,
    control: NativeControl,
    tenant: String,
    workflow: String,
    image: String,
}

fn authority(tenant: &str, caller: Option<&str>) -> ControlAuthority {
    ControlAuthority {
        tenant: tenant.into(),
        caller: caller.map(str::to_owned),
        operation: None,
    }
}

fn query(page_size: u32) -> QueryRequest {
    QueryRequest {
        workflow_id: None,
        run_label: None,
        statuses: vec![],
        parent: None,
        created_after_ms: None,
        created_before_ms: None,
        finished_after_ms: None,
        finished_before_ms: None,
        sort_by: SortField::CreatedAt,
        order: SortOrder::Ascending,
        page_size,
        page_token: None,
    }
}

fn signals(scope: SignalScope, page_size: u32) -> PendingSignalsRequest {
    PendingSignalsRequest {
        scope,
        signal_id: None,
        action_key: None,
        page_size,
        page_token: None,
    }
}

impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("TEST_RUNTARA_DATABASE_URL")
            .or_else(|_| std::env::var("TEST_ENVIRONMENT_DATABASE_URL"))
            .expect("isolated runtime database required");
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        runtara_environment::migrations::run(&pool).await.unwrap();
        let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
        let runtime = Arc::new(RuntimeClient::new(
            Arc::new(EnvironmentHandlerState::new(
                pool.clone(),
                persistence.clone(),
                Arc::new(MockRunner::new()),
                std::env::temp_dir(),
            )),
            RuntimeClientConfig::new(Default::default()),
        ));
        let tenant = format!("control-{}", Uuid::new_v4());
        let control = NativeControl::new(Some(tenant.clone()));
        control.install(runtime);
        let workflow = format!("wf-{}", Uuid::new_v4());
        let image = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO images (image_id, tenant_id, name, binary_path) VALUES ($1,$2,$3,'/test-only')",
        )
        .bind(&image)
        .bind(&tenant)
        .bind(format!("{workflow}:2@fixture"))
        .execute(&pool)
        .await
        .unwrap();
        Self {
            pool,
            persistence,
            control,
            tenant,
            workflow,
            image,
        }
    }

    /// Register a run of the fixture workflow in `tenant`.
    async fn run(&self, name: &str, tenant: &str) -> String {
        let id = format!("{}-{name}", self.tenant);
        self.persistence
            .try_register_instance_with_label(&id, tenant, None, Some(name))
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO instance_images (instance_id, image_id, tenant_id) VALUES ($1,$2,$3)",
        )
        .bind(&id)
        .bind(&self.image)
        .bind(tenant)
        .execute(&self.pool)
        .await
        .unwrap();
        id
    }

    async fn complete(&self, id: &str, status: Core, output: Option<&[u8]>, error: Option<&str>) {
        let mut params = CompleteInstanceParams::new(id, status);
        if let Some(output) = output {
            params = params.with_output(output);
        }
        if let Some(error) = error {
            params = params.with_error(error);
        }
        self.persistence.complete_instance(params).await.unwrap();
    }

    async fn wait_for_signal(&self, id: &str, step: &str, key: Option<&str>, schema: Value) {
        self.persistence
            .update_instance_status(id, Core::Running, None)
            .await
            .unwrap();
        self.persistence
            .input_requests()
            .unwrap()
            .register_input(
                &InputAuthority::Root {
                    tenant_id: self.tenant.clone(),
                    instance_id: id.into(),
                },
                &InputRequestSpec {
                    signal_id: format!("{id}/root/{step}"),
                    response_schema: Some(schema),
                    metadata: json!({"step_id": step, "step_name": "Approve",
                        "action_key": key, "context": {"order": 7}}),
                    deadline: None,
                },
            )
            .await
            .unwrap();
        self.persistence
            .update_instance_status(id, Core::Suspended, None)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE instances SET termination_reason = 'waiting_signal' WHERE instance_id = $1",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn get_reads_the_tenants_runs_with_capped_results() {
    let fx = Fixture::new().await;
    let me = authority(&fx.tenant, None);
    let done = fx.run("done", &fx.tenant).await;
    fx.complete(&done, Core::Completed, Some(br#"{"total":42}"#), None)
        .await;
    let big = fx.run("big", &fx.tenant).await;
    let big_output = format!("\"{}\"", "x".repeat(1024 * 1024));
    fx.complete(&big, Core::Completed, Some(big_output.as_bytes()), None)
        .await;
    let failed = fx.run("failed", &fx.tenant).await;
    fx.complete(&failed, Core::Failed, None, Some("boom")).await;
    let failed_big = fx.run("failed-big", &fx.tenant).await;
    fx.complete(
        &failed_big,
        Core::Failed,
        None,
        Some(&"e".repeat(64 * 1024 + 1)),
    )
    .await;
    let paused = fx.run("paused", &fx.tenant).await;
    fx.persistence
        .update_instance_status(&paused, Core::Suspended, None)
        .await
        .unwrap();
    let foreign = fx.run("foreign", "another-tenant").await;

    let detail = fx.control.get(&me, done.clone()).await.unwrap();
    assert_eq!(detail.instance.status, InstanceStatus::Completed);
    assert_eq!(detail.instance.workflow_id, fx.workflow);
    assert_eq!(detail.instance.version, Some(2));
    assert_eq!(detail.instance.run_label.as_deref(), Some("done"));
    assert!(detail.instance.finished_at_ms.is_some());
    assert_eq!(
        detail.terminal.output.as_deref(),
        Some(br#"{"total":42}"#.as_slice())
    );
    assert!(!detail.terminal.output_omitted);

    let detail = fx.control.get(&me, big).await.unwrap();
    assert!(detail.terminal.output.is_none() && detail.terminal.output_omitted);
    assert_eq!(
        detail.terminal.output_bytes,
        Some(big_output.len() as u64),
        "the size is reported for an omitted output"
    );

    let detail = fx.control.get(&me, failed).await.unwrap();
    assert_eq!(
        detail.terminal.error.as_deref(),
        Some(br#""boom""#.as_slice())
    );
    let detail = fx.control.get(&me, failed_big).await.unwrap();
    assert!(detail.terminal.error.is_none() && detail.terminal.error_omitted);

    let detail = fx.control.get(&me, paused).await.unwrap();
    assert_eq!(detail.instance.status, InstanceStatus::Suspended);
    assert_eq!(
        detail.instance.suspension_reason,
        Some(SuspensionReason::Paused)
    );
    assert!(detail.terminal.output.is_none() && !detail.terminal.output_omitted);

    for missing in [foreign, "no-such-run".into()] {
        assert_eq!(
            fx.control.get(&me, missing).await.unwrap_err().code,
            ControlErrorCode::NotFound
        );
    }
    assert_eq!(
        fx.control.get(&me, " ".into()).await.unwrap_err().code,
        ControlErrorCode::Invalid
    );
    // Another tenant's call is refused before any read.
    assert_eq!(
        fx.control
            .get(&authority("another-tenant", None), done)
            .await
            .unwrap_err()
            .code,
        ControlErrorCode::Denied
    );
}

#[tokio::test]
async fn query_pages_filters_and_sorts_before_paging() {
    let fx = Fixture::new().await;
    let me = authority(&fx.tenant, None);
    let mut ids = Vec::new();
    for index in 0..5 {
        let id = fx.run(&format!("run-{index}"), &fx.tenant).await;
        sqlx::query("UPDATE instances SET created_at = now() + ($2 || ' seconds')::interval WHERE instance_id = $1")
            .bind(&id)
            .bind(index.to_string())
            .execute(&fx.pool)
            .await
            .unwrap();
        ids.push(id);
    }
    fx.complete(&ids[3], Core::Completed, Some(b"{}"), None)
        .await;
    fx.run("elsewhere", "another-tenant").await;

    let mut request = query(2);
    request.workflow_id = Some(fx.workflow.clone());
    let mut seen = Vec::new();
    loop {
        let page = fx.control.query(&me, request.clone()).await.unwrap();
        assert_eq!(page.total, 5);
        seen.extend(page.items.into_iter().map(|item| item.instance_id));
        match page.next_page_token {
            Some(token) => request.page_token = Some(token),
            None => break,
        }
    }
    assert_eq!(seen, ids, "created ascending, tenant only, no duplicates");

    let mut request = query(10);
    request.statuses = vec![InstanceStatus::Completed];
    request.sort_by = SortField::FinishedAt;
    request.order = SortOrder::Descending;
    let page = fx.control.query(&me, request).await.unwrap();
    assert_eq!(
        page.items
            .iter()
            .map(|item| item.instance_id.as_str())
            .collect::<Vec<_>>(),
        [ids[3].as_str()]
    );
    let mut request = query(10);
    request.run_label = Some("run-1".into());
    assert_eq!(fx.control.query(&me, request).await.unwrap().total, 1);
    let mut request = query(10);
    request.statuses = vec![InstanceStatus::Queued, InstanceStatus::NotStarted];
    let page = fx.control.query(&me, request).await.unwrap();
    assert_eq!((page.items.len(), page.total), (0, 0));

    for (request, code) in [
        (query(0), ControlErrorCode::Invalid),
        (query(101), ControlErrorCode::Invalid),
        (
            QueryRequest {
                page_token: Some("next".into()),
                ..query(10)
            },
            ControlErrorCode::Invalid,
        ),
        (
            QueryRequest {
                parent: Some(ParentFilter::Caller),
                ..query(10)
            },
            ControlErrorCode::RequiresInstance,
        ),
        (
            QueryRequest {
                parent: Some(ParentFilter::Instance(ids[0].clone())),
                ..query(10)
            },
            ControlErrorCode::Unsupported,
        ),
    ] {
        assert_eq!(fx.control.query(&me, request).await.unwrap_err().code, code);
    }
    // With a calling run the caller-relative filter waits for its slice.
    let caller = authority(&fx.tenant, Some(&ids[0]));
    let request = QueryRequest {
        parent: Some(ParentFilter::Caller),
        ..query(10)
    };
    assert_eq!(
        fx.control.query(&caller, request).await.unwrap_err().code,
        ControlErrorCode::Unsupported
    );
}

#[tokio::test]
async fn pending_signals_list_open_requests_and_cap_the_page() {
    let fx = Fixture::new().await;
    let me = authority(&fx.tenant, None);
    let first = fx.run("first", &fx.tenant).await;
    fx.wait_for_signal(
        &first,
        "approve",
        Some("finance"),
        json!({"type": "object"}),
    )
    .await;
    let second = fx.run("second", &fx.tenant).await;
    fx.wait_for_signal(&second, "review", None, json!({})).await;

    let page = fx
        .control
        .list_pending_signals(&me, signals(SignalScope::Instance(first.clone()), 10))
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    let signal = &page.items[0];
    assert_eq!(
        (
            signal.instance_id.as_str(),
            signal.workflow_id.as_str(),
            signal.signal_id.as_str(),
            signal.action_key.as_deref()
        ),
        (
            first.as_str(),
            fx.workflow.as_str(),
            "approve",
            Some("finance")
        )
    );
    assert_eq!(
        serde_json::from_slice::<Value>(signal.response_schema.as_ref().unwrap()).unwrap(),
        json!({"type": "object"})
    );
    let context: Value = serde_json::from_slice(signal.context.as_ref().unwrap()).unwrap();
    assert_eq!(context["context"]["order"], 7);
    assert!(page.next_page_token.is_none());

    let page = fx
        .control
        .list_pending_signals(&me, signals(SignalScope::Workflow(fx.workflow.clone()), 1))
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.next_page_token.as_deref(), Some("1"));
    let mut filtered = signals(SignalScope::Workflow(fx.workflow.clone()), 10);
    filtered.signal_id = Some("review".into());
    let page = fx
        .control
        .list_pending_signals(&me, filtered)
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].instance_id, second);
    let mut filtered = signals(SignalScope::Workflow(fx.workflow.clone()), 10);
    filtered.action_key = Some("legal".into());
    assert!(
        fx.control
            .list_pending_signals(&me, filtered)
            .await
            .unwrap()
            .items
            .is_empty()
    );

    let foreign = fx.run("foreign", "another-tenant").await;
    for (request, code) in [
        (
            signals(SignalScope::Instance(foreign), 10),
            ControlErrorCode::NotFound,
        ),
        (
            signals(SignalScope::Children, 10),
            ControlErrorCode::RequiresInstance,
        ),
        (
            signals(SignalScope::Instance(first.clone()), 0),
            ControlErrorCode::Invalid,
        ),
    ] {
        assert_eq!(
            fx.control
                .list_pending_signals(&me, request)
                .await
                .unwrap_err()
                .code,
            code
        );
    }

    // Three requests of 1.5 MiB schemas overflow a 4 MiB page; two fit.
    let heavy = fx.run("heavy", &fx.tenant).await;
    let schema = json!({"description": "s".repeat(1536 * 1024)});
    for step in ["a", "b", "c"] {
        fx.persistence
            .update_instance_status(&heavy, Core::Running, None)
            .await
            .unwrap();
        fx.persistence
            .input_requests()
            .unwrap()
            .register_input(
                &InputAuthority::Root {
                    tenant_id: fx.tenant.clone(),
                    instance_id: heavy.clone(),
                },
                &InputRequestSpec {
                    signal_id: format!("{heavy}/root/{step}"),
                    response_schema: Some(schema.clone()),
                    metadata: json!({"step_id": step}),
                    deadline: None,
                },
            )
            .await
            .unwrap();
    }
    assert_eq!(
        fx.control
            .list_pending_signals(&me, signals(SignalScope::Instance(heavy.clone()), 3))
            .await
            .unwrap_err()
            .code,
        ControlErrorCode::TooLarge
    );
    let page = fx
        .control
        .list_pending_signals(&me, signals(SignalScope::Instance(heavy), 2))
        .await
        .unwrap();
    assert_eq!(page.items.len(), 2);
    assert!(page.next_page_token.is_some());
}

#[tokio::test]
async fn identity_calls_need_a_run_and_the_service_binds_late() {
    let fx = Fixture::new().await;
    let start = || StartRequest {
        workflow_id: fx.workflow.clone(),
        version: None,
        input: b"{}".to_vec(),
        run_label: None,
        parent_close_policy: runtara_component_host::control_host::ParentClosePolicy::Cancel,
    };
    assert_eq!(
        fx.control
            .start(&authority(&fx.tenant, None), start())
            .await
            .unwrap_err()
            .code,
        ControlErrorCode::RequiresInstance
    );
    assert_eq!(
        fx.control
            .start(&authority(&fx.tenant, Some("parent")), start())
            .await
            .unwrap_err()
            .code,
        ControlErrorCode::Unsupported
    );

    // A call before the runtime exists waits for it.
    let late = Arc::new(NativeControl::with_install_wait(
        Some(fx.tenant.clone()),
        Duration::from_secs(10),
    ));
    let done = fx.run("late", &fx.tenant).await;
    let call = {
        let late = late.clone();
        let me = authority(&fx.tenant, None);
        tokio::spawn(async move { late.get(&me, done).await })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    late.install(Arc::new(RuntimeClient::new(
        Arc::new(EnvironmentHandlerState::new(
            fx.pool.clone(),
            fx.persistence.clone(),
            Arc::new(MockRunner::new()),
            std::env::temp_dir(),
        )),
        RuntimeClientConfig::new(Default::default()),
    )));
    assert_eq!(
        call.await.unwrap().unwrap().instance.status,
        InstanceStatus::Pending
    );
}

// ---------------------------------------------------------------------------
// Mutations: identity, decision D1, receipts, audit
// ---------------------------------------------------------------------------

use runtara_component_host::control_host::{CancelRequest, CommandOutcome, SendSignalRequest};
use runtara_server::api::services::control::{Relation, RelationResolver, control_operation_id};

/// Relations a test declares; everything else is `other`, the caller itself.
#[derive(Default)]
struct Lineage(std::collections::HashMap<(String, String), Relation>);

#[async_trait::async_trait]
impl RelationResolver for Lineage {
    async fn relation(
        &self,
        _tenant: &str,
        caller: &str,
        target: &str,
    ) -> Result<Relation, runtara_component_host::control_host::ControlError> {
        Ok(if caller == target {
            Relation::SelfCall
        } else {
            self.0
                .get(&(caller.to_owned(), target.to_owned()))
                .copied()
                .unwrap_or(Relation::Other)
        })
    }
}

fn scoped(tenant: &str, caller: &str, operation: &str) -> ControlAuthority {
    ControlAuthority {
        tenant: tenant.into(),
        caller: Some(caller.into()),
        operation: Some(format!("{:0>64}", operation)),
    }
}

fn signal(instance: &str, step: &str, key: Option<&str>, payload: Value) -> SendSignalRequest {
    SendSignalRequest {
        instance_id: instance.into(),
        signal_id: step.into(),
        action_key: key.map(str::to_owned),
        request_id: None,
        payload: serde_json::to_vec(&payload).unwrap(),
    }
}

impl Fixture {
    /// `self.control` with `relations` and, when given, an audit database.
    fn control_with(&self, relations: Lineage, audit: Option<sqlx::PgPool>) -> NativeControl {
        let mut control =
            NativeControl::new(Some(self.tenant.clone())).with_relations(Arc::new(relations));
        if let Some(pool) = audit {
            control = control.with_audit(pool);
        }
        control.install(Arc::new(RuntimeClient::new(
            Arc::new(EnvironmentHandlerState::new(
                self.pool.clone(),
                self.persistence.clone(),
                Arc::new(MockRunner::new()),
                std::env::temp_dir(),
            )),
            RuntimeClientConfig::new(Default::default()),
        )));
        control
    }

    async fn park(&self, id: &str) {
        self.persistence
            .update_instance_status(id, Core::Suspended, None)
            .await
            .unwrap();
        sqlx::query("UPDATE instances SET termination_reason = 'sleeping', sleep_until = NOW() + INTERVAL '1 hour', wake_reason = 'timer' WHERE instance_id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .unwrap();
    }

    async fn request_state(&self, id: &str) -> (String, Option<String>) {
        sqlx::query_as(
            "SELECT state, operation_id FROM instance_input_requests WHERE instance_id = $1",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }
}

#[tokio::test]
async fn mutations_need_a_run_then_an_operation_scope() {
    let fx = Fixture::new().await;
    let caller = fx.run("caller", &fx.tenant).await;
    let child = fx.run("child", &fx.tenant).await;
    let unscoped = [
        authority(&fx.tenant, None),
        ControlAuthority {
            operation: Some("a".repeat(64)),
            ..authority(&fx.tenant, None)
        },
    ];
    for me in &unscoped {
        assert_eq!(
            fx.control.pause(me, child.clone()).await.unwrap_err().code,
            ControlErrorCode::RequiresInstance
        );
    }
    let no_operation = authority(&fx.tenant, Some(&caller));
    for code in [
        fx.control
            .send_signal(&no_operation, signal(&child, "approve", None, json!({})))
            .await
            .unwrap_err()
            .code,
        fx.control
            .cancel(
                &no_operation,
                CancelRequest {
                    instance_id: child.clone(),
                    reason: None,
                    grace_ms: None,
                },
            )
            .await
            .unwrap_err()
            .code,
        fx.control
            .pause(&no_operation, child.clone())
            .await
            .unwrap_err()
            .code,
        fx.control
            .resume(&no_operation, child.clone())
            .await
            .unwrap_err()
            .code,
    ] {
        assert_eq!(code, ControlErrorCode::RequiresOperation);
    }
}

/// Decision D1 for the lifecycle commands, and their outcomes on a child:
/// a waiting child pauses at once (D4), a paused child resumes, a parked
/// child cancels at once; each replay of an operation answers from its
/// receipt and changed arguments conflict.
#[tokio::test]
async fn lifecycle_commands_reach_children_only_and_replay_from_receipts() {
    let fx = Fixture::new().await;
    let caller = fx.run("caller", &fx.tenant).await;
    let child = fx.run("child", &fx.tenant).await;
    let stranger = fx.run("stranger", &fx.tenant).await;
    let parent = fx.run("parent", &fx.tenant).await;
    let mut lineage = Lineage::default();
    lineage
        .0
        .insert((caller.clone(), child.clone()), Relation::Child);
    lineage
        .0
        .insert((caller.clone(), parent.clone()), Relation::Ancestor);
    let control = fx.control_with(lineage, None);

    // Until slice 7 the real service relates every other run as `other`.
    for (target, code) in [
        (child.clone(), ControlErrorCode::NotChild),
        (caller.clone(), ControlErrorCode::Invalid),
    ] {
        assert_eq!(
            fx.control
                .pause(&scoped(&fx.tenant, &caller, "1"), target)
                .await
                .unwrap_err()
                .code,
            code
        );
    }
    for (target, code) in [
        (stranger.clone(), ControlErrorCode::NotChild),
        (parent.clone(), ControlErrorCode::Denied),
        (caller.clone(), ControlErrorCode::Invalid),
    ] {
        assert_eq!(
            control
                .resume(&scoped(&fx.tenant, &caller, "2"), target)
                .await
                .unwrap_err()
                .code,
            code
        );
    }

    // Pause a waiting child: immediate (D4).
    fx.park(&child).await;
    let paused = control
        .pause(&scoped(&fx.tenant, &caller, "3"), child.clone())
        .await
        .unwrap();
    assert_eq!(
        (paused.outcome, paused.replayed),
        (CommandOutcome::Applied, false)
    );
    let detail = control
        .get(&authority(&fx.tenant, None), child.clone())
        .await
        .unwrap();
    assert_eq!(
        detail.instance.suspension_reason,
        Some(SuspensionReason::Paused)
    );
    // The same operation replays from its receipt; another target conflicts.
    let replay = control
        .pause(&scoped(&fx.tenant, &caller, "3"), child.clone())
        .await
        .unwrap();
    assert_eq!(
        (replay.outcome, replay.replayed),
        (CommandOutcome::Applied, true)
    );
    let stranger_child = fx.run("other-child", &fx.tenant).await;
    let mut lineage = Lineage::default();
    lineage
        .0
        .insert((caller.clone(), child.clone()), Relation::Child);
    lineage
        .0
        .insert((caller.clone(), stranger_child.clone()), Relation::Child);
    let control = fx.control_with(lineage, None);
    assert_eq!(
        control
            .pause(&scoped(&fx.tenant, &caller, "3"), stranger_child.clone())
            .await
            .unwrap_err()
            .code,
        ControlErrorCode::ReplayConflict
    );
    assert_eq!(
        control
            .pause(&scoped(&fx.tenant, &caller, "4"), child.clone())
            .await
            .unwrap()
            .outcome,
        CommandOutcome::Unchanged
    );

    // Resume: only an explicitly paused child relaunches.
    fx.park(&stranger_child).await;
    assert_eq!(
        control
            .resume(&scoped(&fx.tenant, &caller, "5"), stranger_child.clone())
            .await
            .unwrap_err()
            .code,
        ControlErrorCode::NotPaused,
        "a waiting child is not resumed early"
    );
    let resumed = control
        .resume(&scoped(&fx.tenant, &caller, "6"), child.clone())
        .await
        .unwrap();
    assert_eq!(resumed.outcome, CommandOutcome::Applied);
    let kind: String = sqlx::query_scalar(
        "SELECT kind FROM instance_launches WHERE instance_id = $1 AND state = 'queued'",
    )
    .bind(&child)
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    assert_eq!(kind, "resume");

    // Cancel a parked child with a reason: it ends at once and the command
    // carries the reason; a finished child is already terminal.
    let invalid = control
        .cancel(
            &scoped(&fx.tenant, &caller, "7"),
            CancelRequest {
                instance_id: stranger_child.clone(),
                reason: None,
                grace_ms: Some(3_600_001),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(invalid.code, ControlErrorCode::Invalid);
    let cancelled = control
        .cancel(
            &scoped(&fx.tenant, &caller, "8"),
            CancelRequest {
                instance_id: stranger_child.clone(),
                reason: Some("approval withdrawn".into()),
                grace_ms: Some(0),
            },
        )
        .await
        .unwrap();
    assert_eq!(cancelled.outcome, CommandOutcome::Applied);
    let (status, payload): (String, Option<Vec<u8>>) = sqlx::query_as(
        "SELECT i.status::text, s.payload FROM instances i JOIN pending_signals s USING (instance_id) WHERE i.instance_id = $1",
    )
    .bind(&stranger_child)
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    assert_eq!(status, "cancelled");
    assert_eq!(payload.as_deref(), Some(b"approval withdrawn".as_slice()));
    let again = control
        .cancel(
            &scoped(&fx.tenant, &caller, "9"),
            CancelRequest {
                instance_id: stranger_child.clone(),
                reason: None,
                grace_ms: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(again.outcome, CommandOutcome::AlreadyTerminal);
    // A terminal child cannot pause; the failed attempt leaves no receipt.
    assert_eq!(
        control
            .pause(&scoped(&fx.tenant, &caller, "10"), stranger_child.clone())
            .await
            .unwrap_err()
            .code,
        ControlErrorCode::NotPausable
    );
    let receipts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM instance_control_receipts WHERE caller_instance_id = $1 AND state = 'pending'",
    )
    .bind(&caller)
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    assert_eq!(receipts, 0, "receipts are success-only");
}

/// `send-signal` answers the one open request of a step, opted in with
/// `action.key` outside the lineage, once per operation: a replay returns the
/// same request, changed arguments conflict, and a crash between the answer
/// and its receipt re-applies without answering twice.
#[tokio::test]
async fn send_signal_answers_once_per_operation() {
    let fx = Fixture::new().await;
    let caller = fx.run("caller", &fx.tenant).await;
    let target = fx.run("approver", &fx.tenant).await;
    fx.wait_for_signal(
        &target,
        "approve",
        Some("finance"),
        json!({"approved": {"type": "boolean", "required": true}}),
    )
    .await;
    let control = fx.control_with(Lineage::default(), None);
    let me = scoped(&fx.tenant, &caller, "1");

    for (request, code) in [
        (
            signal(&target, "approve", None, json!({"approved": true})),
            ControlErrorCode::Denied,
        ),
        (
            signal(&target, "approve", Some("legal"), json!({"approved": true})),
            ControlErrorCode::NotWaiting,
        ),
        (
            signal(
                &target,
                "reject",
                Some("finance"),
                json!({"approved": true}),
            ),
            ControlErrorCode::NotWaiting,
        ),
        (
            signal(
                &caller,
                "approve",
                Some("finance"),
                json!({"approved": true}),
            ),
            ControlErrorCode::Invalid,
        ),
        (
            signal(
                &target,
                "approve",
                Some("finance"),
                json!({"approved": "yes"}),
            ),
            ControlErrorCode::Invalid,
        ),
    ] {
        assert_eq!(
            control.send_signal(&me, request).await.unwrap_err().code,
            code
        );
    }
    assert_eq!(fx.request_state(&target).await.0, "open");

    let sent = control
        .send_signal(
            &me,
            signal(
                &target,
                "approve",
                Some("finance"),
                json!({"approved": true}),
            ),
        )
        .await
        .unwrap();
    assert!(!sent.replayed);
    let (state, operation) = fx.request_state(&target).await;
    assert_eq!(state, "accepted");
    assert_eq!(
        operation.as_deref(),
        Some(control_operation_id(&caller, me.operation.as_deref().unwrap()).as_str())
    );

    let replay = control
        .send_signal(
            &me,
            signal(
                &target,
                "approve",
                Some("finance"),
                json!({"approved": true}),
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        (replay.request_id.as_str(), replay.replayed),
        (sent.request_id.as_str(), true)
    );
    assert_eq!(
        control
            .send_signal(
                &me,
                signal(
                    &target,
                    "approve",
                    Some("finance"),
                    json!({"approved": false})
                ),
            )
            .await
            .unwrap_err()
            .code,
        ControlErrorCode::ReplayConflict
    );
    // Another operation finds nothing open.
    assert_eq!(
        control
            .send_signal(
                &scoped(&fx.tenant, &caller, "2"),
                signal(
                    &target,
                    "approve",
                    Some("finance"),
                    json!({"approved": true})
                ),
            )
            .await
            .unwrap_err()
            .code,
        ControlErrorCode::NotWaiting
    );

    // A crash after the answer but before its receipt completed.
    sqlx::query("UPDATE instance_control_receipts SET state = 'pending', result = NULL, completed_at = NULL WHERE caller_instance_id = $1")
        .bind(&caller)
        .execute(&fx.pool)
        .await
        .unwrap();
    let reapplied = control
        .send_signal(
            &me,
            signal(
                &target,
                "approve",
                Some("finance"),
                json!({"approved": true}),
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        (reapplied.request_id.as_str(), reapplied.replayed),
        (sent.request_id.as_str(), true),
        "the re-application finds its own answer instead of answering again"
    );
    let state: String = sqlx::query_scalar(
        "SELECT state FROM instance_control_receipts WHERE caller_instance_id = $1",
    )
    .bind(&caller)
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    assert_eq!(state, "completed");

    // Several open requests of one step are ambiguous without a request id.
    let looped = fx.run("looped", &fx.tenant).await;
    for iteration in ["a", "b"] {
        fx.persistence
            .update_instance_status(&looped, Core::Running, None)
            .await
            .unwrap();
        fx.persistence
            .input_requests()
            .unwrap()
            .register_input(
                &InputAuthority::Root {
                    tenant_id: fx.tenant.clone(),
                    instance_id: looped.clone(),
                },
                &InputRequestSpec {
                    signal_id: format!("{looped}/loop/{iteration}/approve"),
                    response_schema: None,
                    metadata: json!({"step_id": "approve", "action_key": "finance"}),
                    deadline: None,
                },
            )
            .await
            .unwrap();
    }
    let ambiguous = signal(&looped, "approve", Some("finance"), json!({}));
    assert_eq!(
        control
            .send_signal(&scoped(&fx.tenant, &caller, "3"), ambiguous)
            .await
            .unwrap_err()
            .code,
        ControlErrorCode::Ambiguous
    );
    let first = fx
        .control
        .list_pending_signals(
            &authority(&fx.tenant, None),
            signals(SignalScope::Instance(looped.clone()), 10),
        )
        .await
        .unwrap()
        .items
        .remove(0);
    let mut picked = signal(&looped, "approve", Some("finance"), json!({}));
    picked.request_id = Some(first.request_id.clone());
    assert_eq!(
        control
            .send_signal(&scoped(&fx.tenant, &caller, "4"), picked)
            .await
            .unwrap()
            .request_id,
        first.request_id
    );
}

/// Public submission paths refuse control's reserved `control:` space.
#[tokio::test]
async fn public_submissions_refuse_the_control_prefix() {
    let fx = Fixture::new().await;
    let target = fx.run("approver", &fx.tenant).await;
    fx.wait_for_signal(&target, "approve", None, json!({}))
        .await;
    let runtime = RuntimeClient::new(
        Arc::new(EnvironmentHandlerState::new(
            fx.pool.clone(),
            fx.persistence.clone(),
            Arc::new(MockRunner::new()),
            std::env::temp_dir(),
        )),
        RuntimeClientConfig::new(Default::default()),
    );
    let request = runtara_core::persistence::inputs::request_id(&format!("{target}/root/approve"));
    assert_eq!(
        runtime
            .submit_input_response(&fx.tenant, &target, &request, "control:forged", &json!({}),)
            .await
            .unwrap_err(),
        runtara_core::persistence::inputs::InputError::InvalidRequest
    );
    assert_eq!(fx.request_state(&target).await.0, "open");
    runtime
        .submit_input_response(&fx.tenant, &target, &request, "ui-1", &json!({}))
        .await
        .unwrap();
}

/// The public API's pause and resume: a waiting run pauses at once (D4), and
/// failed or cancelled runs are not resumable.
#[tokio::test]
async fn public_pause_is_immediate_for_waiting_runs_and_resume_refuses_terminal_runs() {
    use runtara_server::workers::execution_engine::{
        CommandEffect, PauseOutcome, ResumeOutcome, pause_for, resume_for,
    };
    let fx = Fixture::new().await;
    let runtime = RuntimeClient::new(
        Arc::new(EnvironmentHandlerState::new(
            fx.pool.clone(),
            fx.persistence.clone(),
            Arc::new(MockRunner::new()),
            std::env::temp_dir(),
        )),
        RuntimeClientConfig::new(Default::default()),
    );
    let waiting = fx.run("waiting", &fx.tenant).await;
    fx.wait_for_signal(&waiting, "approve", None, json!({}))
        .await;
    assert!(matches!(
        pause_for(&runtime, &fx.tenant, &waiting).await.unwrap(),
        PauseOutcome::Paused {
            effect: CommandEffect::Applied,
            ..
        }
    ));
    assert!(matches!(
        pause_for(&runtime, &fx.tenant, &waiting).await.unwrap(),
        PauseOutcome::AlreadyPaused
    ));
    // The public resume relaunches any suspended run.
    assert!(matches!(
        resume_for(&runtime, &fx.tenant, &waiting, false)
            .await
            .unwrap(),
        ResumeOutcome::Resumed { .. }
    ));
    for status in [Core::Failed, Core::Cancelled] {
        let done = fx.run(&format!("{status:?}"), &fx.tenant).await;
        fx.complete(&done, status, None, None).await;
        assert!(matches!(
            resume_for(&runtime, &fx.tenant, &done, false)
                .await
                .unwrap(),
            ResumeOutcome::NotResumable { .. }
        ));
    }
    // Another tenant's run is not found.
    let foreign = fx.run("foreign", "another-tenant").await;
    assert!(pause_for(&runtime, &fx.tenant, &foreign).await.is_err());
}

/// Every mutation attempt is audited, successes and refusals alike, without
/// the signal payload.
#[tokio::test]
async fn mutations_are_audited_without_payloads() {
    static SERVER_MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
    let fx = Fixture::new().await;
    let url = std::env::var("TEST_RUNTARA_SERVER_DATABASE_URL")
        .expect("the audit test needs TEST_RUNTARA_SERVER_DATABASE_URL");
    let audit = sqlx::PgPool::connect(&url).await.unwrap();
    SERVER_MIGRATOR.run(&audit).await.unwrap();
    let caller = fx.run("caller", &fx.tenant).await;
    let target = fx.run("approver", &fx.tenant).await;
    fx.wait_for_signal(&target, "approve", Some("finance"), json!({}))
        .await;
    let control = fx.control_with(Lineage::default(), Some(audit.clone()));
    control
        .send_signal(
            &scoped(&fx.tenant, &caller, "1"),
            signal(
                &target,
                "approve",
                Some("finance"),
                json!({"secret": "s3cr3t"}),
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        control
            .cancel(
                &scoped(&fx.tenant, &caller, "2"),
                CancelRequest {
                    instance_id: target.clone(),
                    reason: None,
                    grace_ms: None,
                },
            )
            .await
            .unwrap_err()
            .code,
        ControlErrorCode::NotChild
    );
    let rows: Vec<(String, Option<String>, Value)> = sqlx::query_as(
        "SELECT event_type, resource_id, payload FROM audit_events WHERE tenant_id = $1 ORDER BY created_at",
    )
    .bind(&fx.tenant)
    .fetch_all(&audit)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0].0, "control.send_signal");
    assert_eq!(rows[0].1.as_deref(), Some(target.as_str()));
    assert_eq!(rows[0].2["callerInstanceId"], caller);
    assert_eq!(rows[0].2["outcome"], "accepted");
    assert_eq!(rows[1].0, "control.cancel");
    assert_eq!(rows[1].2["error"], "not-child");
    for (_, _, payload) in &rows {
        assert!(!payload.to_string().contains("s3cr3t"), "{payload}");
    }
}
