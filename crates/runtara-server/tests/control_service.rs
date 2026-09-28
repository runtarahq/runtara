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

/// A database beside the runtime test database, created on first use and
/// migrated with the combined core and Environment migrations. The runtime
/// test database itself belongs to the core-only suite (`core_instance_api`),
/// whose migrator refuses a `_sqlx_migrations` table holding Environment rows.
async fn migrated_pool() -> sqlx::PgPool {
    use sqlx::{ConnectOptions, Executor};
    let url = std::env::var("TEST_RUNTARA_DATABASE_URL")
        .or_else(|_| std::env::var("TEST_ENVIRONMENT_DATABASE_URL"))
        .expect("isolated runtime database required");
    let base: sqlx::postgres::PgConnectOptions = url
        .parse()
        .expect("the runtime test database URL must parse");
    let name = format!(
        "{}_control_service",
        base.get_database().unwrap_or("runtara_test")
    );
    // A duplicate-database error means an earlier run created it already.
    let mut admin = base
        .clone()
        .database("postgres")
        .connect()
        .await
        .expect("the test database server must accept connections");
    let _ = admin
        .execute(format!("CREATE DATABASE \"{name}\"").as_str())
        .await;
    let pool = sqlx::PgPool::connect_with(base.database(&name))
        .await
        .expect("the derived test database must accept connections");
    runtara_environment::migrations::run(&pool)
        .await
        .expect("core and Environment migrations must succeed");
    pool
}

impl Fixture {
    async fn new() -> Self {
        let pool = migrated_pool().await;
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
                parent: Some(ParentFilter::Instance(" ".into())),
                ..query(10)
            },
            ControlErrorCode::Invalid,
        ),
    ] {
        assert_eq!(fx.control.query(&me, request).await.unwrap_err().code, code);
    }
    // A run without children has an empty page, by id or as the caller.
    let caller = authority(&fx.tenant, Some(&ids[0]));
    for (me, parent) in [
        (&me, ParentFilter::Instance(ids[0].clone())),
        (&caller, ParentFilter::Caller),
    ] {
        let page = fx
            .control
            .query(
                me,
                QueryRequest {
                    parent: Some(parent),
                    ..query(10)
                },
            )
            .await
            .unwrap();
        assert_eq!((page.items.len(), page.total), (0, 0));
    }
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
        ControlErrorCode::RequiresOperation
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

// ---------------------------------------------------------------------------
// start: parent link, idempotent admission, capacity, parent-aware reads
// ---------------------------------------------------------------------------

use runtara_component_host::control_host::ParentClosePolicy;
use runtara_core::persistence::ParentLink;
use runtara_server::api::repositories::workflows::WorkflowRepository;
use runtara_server::workers::execution_engine::ExecutionEngine;

/// The concurrency limit every test of this binary runs under: control's
/// share of it is `max(1, floor(0.8 x 3))` = 2.
const LIMIT: &str = "3";

static CONFIG: std::sync::Once = std::sync::Once::new();

fn init_config() {
    CONFIG.call_once(|| {
        // SAFETY: set once, before any test reads the environment.
        unsafe {
            std::env::set_var("MAX_CONCURRENT_EXECUTIONS", LIMIT);
            if std::env::var("TENANT_ID").is_err() {
                std::env::set_var("TENANT_ID", "control-service-tests");
            }
            if std::env::var("RUNTARA_MCP_SESSION_STORE").is_err() {
                std::env::set_var("RUNTARA_MCP_SESSION_STORE", "local");
            }
            if std::env::var("OBJECT_MODEL_DATABASE_URL").is_err() {
                std::env::set_var("OBJECT_MODEL_DATABASE_URL", "postgres://unused/unused");
            }
        }
        runtara_server::config::init(
            runtara_server::config::Config::from_env().expect("test configuration"),
        );
    });
}

struct Children {
    fx: Fixture,
    server: sqlx::PgPool,
    engine: Arc<ExecutionEngine>,
    control: NativeControl,
    runtime: Arc<RuntimeClient>,
}

static SERVER_MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

impl Children {
    async fn new() -> Self {
        init_config();
        let fx = Fixture::new().await;
        let url = std::env::var("TEST_RUNTARA_SERVER_DATABASE_URL")
            .expect("start tests need TEST_RUNTARA_SERVER_DATABASE_URL");
        let server = sqlx::PgPool::connect(&url).await.unwrap();
        SERVER_MIGRATOR.run(&server).await.unwrap();
        let runtime = Arc::new(RuntimeClient::new(
            Arc::new(EnvironmentHandlerState::new(
                fx.pool.clone(),
                fx.persistence.clone(),
                Arc::new(MockRunner::new()),
                std::env::temp_dir(),
            )),
            RuntimeClientConfig::new(Default::default()),
        ));
        let (events, _) = tokio::sync::mpsc::channel(64);
        let engine = Arc::new(ExecutionEngine::new(
            server.clone(),
            Arc::new(WorkflowRepository::new(server.clone())),
            Some(runtime.clone()),
            None,
            runtara_server::product_events::ProductEventSink::new(events),
        ));
        let control = NativeControl::new(Some(fx.tenant.clone()));
        control.install(runtime.clone());
        control.install_engine(engine.clone());
        Self {
            fx,
            server,
            engine,
            control,
            runtime,
        }
    }

    /// A workflow of the tenant whose single version takes `{order: integer}`.
    async fn workflow(&self, definition: Value) -> String {
        let workflow = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO workflows (tenant_id, workflow_id, version_count, latest_version) VALUES ($1, $2, 1, 1)",
        )
        .bind(&self.fx.tenant)
        .bind(&workflow)
        .execute(&self.server)
        .await
        .unwrap();
        let size = serde_json::to_vec(&definition).unwrap().len() as i32;
        sqlx::query(
            "INSERT INTO workflow_definitions (tenant_id, workflow_id, version, definition, file_size, track_events) \
             VALUES ($1, $2, 1, $3, $4, false)",
        )
        .bind(&self.fx.tenant)
        .bind(&workflow)
        .bind(&definition)
        .bind(size)
        .execute(&self.server)
        .await
        .unwrap();
        workflow
    }

    async fn child_workflow(&self) -> String {
        self.workflow(json!({
            "name": "child", "steps": {}, "executionPlan": [], "entryPoint": null,
            "variables": {}, "outputSchema": {},
            "inputSchema": {"order": {"type": "integer", "required": true}}
        }))
        .await
    }

    /// Launch an admitted child the way Environment does: an instance row
    /// carrying the parent link and the admission time.
    async fn launch(&self, child: &str) {
        let request = self
            .engine
            .control_child(&self.fx.tenant, child)
            .await
            .unwrap()
            .expect("an admitted child");
        assert!(
            self.fx
                .persistence
                .try_register_child_instance(
                    child,
                    &self.fx.tenant,
                    None,
                    request.run_label.as_deref(),
                    &ParentLink {
                        parent_instance_id: request.parent_instance_id.unwrap(),
                        parent_close_policy: request.parent_close_policy.unwrap(),
                        admitted_at: request.created_at,
                    },
                )
                .await
                .unwrap()
        );
        sqlx::query(
            "INSERT INTO instance_images (instance_id, image_id, tenant_id) VALUES ($1,$2,$3)",
        )
        .bind(child)
        .bind(&self.fx.image)
        .bind(&self.fx.tenant)
        .execute(&self.fx.pool)
        .await
        .unwrap();
    }

    async fn cleanup(&self) {
        for sql in [
            "DELETE FROM execution_outbox WHERE request_id IN (SELECT request_id FROM execution_requests WHERE tenant_id = $1)",
            "DELETE FROM execution_admission_reservations WHERE tenant_id = $1",
            "DELETE FROM execution_requests WHERE tenant_id = $1",
            "DELETE FROM execution_admission_tenants WHERE tenant_id = $1",
        ] {
            sqlx::query(sql)
                .bind(&self.fx.tenant)
                .execute(&self.server)
                .await
                .unwrap();
        }
    }
}

fn start_request(workflow: &str, order: i64, label: Option<&str>) -> StartRequest {
    StartRequest {
        workflow_id: workflow.into(),
        version: None,
        input: serde_json::to_vec(&json!({"data": {"order": order}, "variables": {}})).unwrap(),
        run_label: label.map(str::to_owned),
        parent_close_policy: ParentClosePolicy::Cancel,
    }
}

/// `start` admits children durably and idempotently; `get`, `query(parent)`
/// and the relations see them in admission and after launch; the lifecycle
/// commands reach real children and nothing else.
#[tokio::test]
async fn start_admits_children_that_get_query_and_lifecycle_follow() {
    let cx = Children::new().await;
    let tenant = cx.fx.tenant.clone();
    let workflow = cx.child_workflow().await;
    let parent = cx.fx.run("parent", &tenant).await;
    let op = |n: &str| scoped(&tenant, &parent, n);

    let first = cx
        .control
        .start(&op("1"), start_request(&workflow, 1, Some("order-1")))
        .await
        .unwrap();
    assert!(!first.replayed);
    assert_eq!(
        (
            first.workflow_id.as_str(),
            first.version,
            first.run_label.as_deref()
        ),
        (workflow.as_str(), 1, Some("order-1"))
    );
    // The same operation replays the same child; other arguments conflict.
    let replay = cx
        .control
        .start(&op("1"), start_request(&workflow, 1, Some("order-1")))
        .await
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.instance_id, first.instance_id);
    let codes = [
        cx.control
            .start(&op("1"), start_request(&workflow, 2, Some("order-1")))
            .await
            .unwrap_err()
            .code,
        cx.control
            .start(&op("2"), start_request(&workflow, 2, Some("order-1")))
            .await
            .unwrap_err()
            .code,
    ];
    assert_eq!(
        codes,
        [
            ControlErrorCode::ReplayConflict,
            ControlErrorCode::LabelConflict
        ]
    );

    // In admission, the child is `queued` and names its parent.
    let queued = cx
        .control
        .get(&authority(&tenant, None), first.instance_id.clone())
        .await
        .unwrap();
    assert_eq!(queued.instance.status, InstanceStatus::Queued);
    assert_eq!(
        queued.instance.parent_instance_id.as_deref(),
        Some(parent.as_str())
    );
    assert_eq!(queued.instance.version, Some(1));
    // It cannot pause or resume before it starts.
    for (result, code) in [
        (
            cx.control.pause(&op("p0"), first.instance_id.clone()).await,
            ControlErrorCode::NotPausable,
        ),
        (
            cx.control
                .resume(&op("r0"), first.instance_id.clone())
                .await,
            ControlErrorCode::NotPaused,
        ),
    ] {
        assert_eq!(result.unwrap_err().code, code);
    }

    // A second child launches.
    let second = cx
        .control
        .start(&op("3"), start_request(&workflow, 3, None))
        .await
        .unwrap();
    cx.launch(&second.instance_id).await;
    let launched = cx
        .control
        .get(&authority(&tenant, None), second.instance_id.clone())
        .await
        .unwrap();
    assert_eq!(launched.instance.status, InstanceStatus::Pending);
    assert_eq!(
        launched.instance.parent_instance_id.as_deref(),
        Some(parent.as_str())
    );

    // query(parent) merges both, pages by admission, and filters by state.
    let mut request = QueryRequest {
        parent: Some(ParentFilter::Caller),
        ..query(1)
    };
    let mut seen = Vec::new();
    loop {
        let page = cx.control.query(&op("q"), request.clone()).await.unwrap();
        assert_eq!(page.total, 2);
        seen.extend(page.items);
        match page.next_page_token {
            Some(token) => request.page_token = Some(token),
            None => break,
        }
    }
    assert_eq!(
        seen.iter()
            .map(|item| (item.instance_id.as_str(), item.status))
            .collect::<Vec<_>>(),
        [
            (first.instance_id.as_str(), InstanceStatus::Queued),
            (second.instance_id.as_str(), InstanceStatus::Pending)
        ]
    );
    for (statuses, expected) in [
        (vec![InstanceStatus::Queued], &first.instance_id),
        (vec![InstanceStatus::Pending], &second.instance_id),
    ] {
        let page = cx
            .control
            .query(
                &authority(&tenant, None),
                QueryRequest {
                    parent: Some(ParentFilter::Instance(parent.clone())),
                    statuses,
                    ..query(10)
                },
            )
            .await
            .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(&page.items[0].instance_id, expected);
    }
    let mut labelled = QueryRequest {
        parent: Some(ParentFilter::Instance(parent.clone())),
        ..query(10)
    };
    labelled.run_label = Some("order-1".into());
    assert_eq!(
        cx.control
            .query(&authority(&tenant, None), labelled)
            .await
            .unwrap()
            .total,
        1
    );

    // Relations come from the parent links: the lifecycle commands reach the
    // launched child, the child cannot command its parent, and a stranger
    // cannot command either.
    cx.fx.park(&second.instance_id).await;
    let paused = cx
        .control
        .pause(&op("p1"), second.instance_id.clone())
        .await
        .unwrap();
    assert_eq!(paused.outcome, CommandOutcome::Applied);
    let resumed = cx
        .control
        .resume(&op("r1"), second.instance_id.clone())
        .await
        .unwrap();
    assert_eq!(resumed.outcome, CommandOutcome::Applied);
    let child_calls = scoped(&tenant, &second.instance_id, "c1");
    assert_eq!(
        cx.control
            .pause(&child_calls, parent.clone())
            .await
            .unwrap_err()
            .code,
        ControlErrorCode::Denied,
        "a parent is an ancestor of its child"
    );
    // send-signal may reach an ancestor: it gets past authorization.
    assert_eq!(
        cx.control
            .send_signal(
                &scoped(&tenant, &second.instance_id, "s1"),
                signal(&parent, "approve", None, json!({}))
            )
            .await
            .unwrap_err()
            .code,
        ControlErrorCode::NotWaiting
    );
    let stranger = cx.fx.run("stranger", &tenant).await;
    assert_eq!(
        cx.control
            .pause(&scoped(&tenant, &stranger, "x"), second.instance_id.clone())
            .await
            .unwrap_err()
            .code,
        ControlErrorCode::NotChild
    );
    let cancelled = cx
        .control
        .cancel(
            &op("c2"),
            CancelRequest {
                instance_id: second.instance_id.clone(),
                reason: None,
                grace_ms: Some(0),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        cancelled.outcome,
        CommandOutcome::Applied | CommandOutcome::Requested
    ));

    // The public list filters by parent too.
    let page = cx
        .engine
        .list_all_executions(
            &tenant,
            None,
            None,
            runtara_server::api::dto::executions::ExecutionFilters {
                parent_instance_id: Some(parent.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(page.content.len(), 1);
    assert_eq!(
        page.content[0].parent_instance_id.as_deref(),
        Some(parent.as_str())
    );

    // Refused starts and replays leaked no reservation: one per child.
    let held: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM execution_admission_reservations WHERE tenant_id = $1 AND released_at IS NULL",
    )
    .bind(&tenant)
    .fetch_one(&cx.server)
    .await
    .unwrap();
    assert_eq!(held, 2);
    cx.cleanup().await;
}

/// Decisions D5-D7: the control share is retryable capacity with a hint; a
/// missing workflow or version is `not-found`; a permanently failed compile
/// is `not-runnable` and admits nothing, while a workflow not compiled yet
/// is admitted; inputs are checked; lineage stops at depth 16.
#[tokio::test]
async fn start_refuses_what_can_never_run_and_what_does_not_fit() {
    let cx = Children::new().await;
    let tenant = cx.fx.tenant.clone();
    let workflow = cx.child_workflow().await;
    let parent = cx.fx.run("parent", &tenant).await;
    // Park the parent so the concurrency gate counts only children.
    cx.fx.park(&parent).await;
    let op = |n: &str| scoped(&tenant, &parent, n);
    let code = |result: Result<_, runtara_component_host::control_host::ControlError>| {
        result
            .map(|_: runtara_component_host::control_host::StartResult| ())
            .unwrap_err()
    };

    let missing = code(
        cx.control
            .start(&op("m1"), start_request("no-such-workflow", 1, None))
            .await,
    );
    assert_eq!(missing.code, ControlErrorCode::NotFound);
    let mut versioned = start_request(&workflow, 1, None);
    versioned.version = Some(9);
    assert_eq!(
        code(cx.control.start(&op("m2"), versioned).await).code,
        ControlErrorCode::NotFound
    );
    let mut bad_inputs = start_request(&workflow, 1, None);
    bad_inputs.input = br#"{"data": {"order": "seven"}}"#.to_vec();
    assert_eq!(
        code(cx.control.start(&op("m3"), bad_inputs).await).code,
        ControlErrorCode::Invalid
    );
    let mut malformed = start_request(&workflow, 1, None);
    malformed.input = b"[1]".to_vec();
    assert_eq!(
        code(cx.control.start(&op("m4"), malformed).await).code,
        ControlErrorCode::Invalid
    );

    // A definition whose compile failed for good is not runnable; nothing
    // is admitted for it.
    let broken_definition = json!({
        "name": "broken", "steps": {}, "executionPlan": [], "entryPoint": null
    });
    let broken = cx.workflow(broken_definition.clone()).await;
    sqlx::query(
        "INSERT INTO workflow_compilations
            (tenant_id, workflow_id, version, compilation_status, translated_path,
             error_message, source_checksum, track_events, template_major, lowering_mode,
             compiler_build)
         VALUES ($1, $2, 1, 'failed', '', '[E004] Workflow has no steps defined', $3, false, $4, $5, $6)",
    )
    .bind(&tenant)
    .bind(&broken)
    .bind(runtara_server::api::repositories::workflows::workflow_definition_checksum(
        &broken_definition,
    ))
    .bind(runtara_workflows::TEMPLATE_MAJOR_VERSION)
    .bind(runtara_server::config::workflow_lowering_tag())
    .bind(runtara_server::api::repositories::workflows::compiler_build_id())
    .execute(&cx.server)
    .await
    .unwrap();
    let not_runnable = code(
        cx.control
            .start(&op("b1"), start_request(&broken, 1, None))
            .await,
    );
    assert_eq!(not_runnable.code, ControlErrorCode::NotRunnable);
    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM execution_requests WHERE tenant_id = $1 AND workflow_id = $2",
    )
    .bind(&tenant)
    .bind(&broken)
    .fetch_one(&cx.server)
    .await
    .unwrap();
    assert_eq!(rows, 0, "a not-runnable start writes nothing");

    // Not compiled yet: admitted, up to control's share of the limit.
    for n in ["c1", "c2"] {
        cx.control
            .start(&op(n), start_request(&workflow, 1, None))
            .await
            .unwrap_or_else(|error| panic!("{n}: {error:?}"));
    }
    let full = code(
        cx.control
            .start(&op("c3"), start_request(&workflow, 1, None))
            .await,
    );
    assert_eq!(full.code, ControlErrorCode::Capacity);
    let hint = full.retry_after_ms.expect("a full share is retryable");
    assert!((3_000..=8_000).contains(&hint), "{hint}");
    // A replay of an admitted operation is not capacity.
    assert!(
        cx.control
            .start(&op("c1"), start_request(&workflow, 1, None))
            .await
            .unwrap()
            .replayed
    );

    // Lineage: a run at depth 16 cannot start a child; depth 15 can.
    let mut chain = vec![cx.fx.run("depth-1", &tenant).await];
    for depth in 2..=16 {
        let id = format!("{tenant}-depth-{depth}");
        cx.fx
            .persistence
            .try_register_child_instance(
                &id,
                &tenant,
                None,
                None,
                &ParentLink {
                    parent_instance_id: chain.last().unwrap().clone(),
                    parent_close_policy: "cancel".into(),
                    admitted_at: chrono::Utc::now(),
                },
            )
            .await
            .unwrap();
        chain.push(id);
    }
    for id in &chain {
        cx.fx.park(id).await;
    }
    let too_deep = code(
        cx.control
            .start(
                &scoped(&tenant, &chain[15], "d16"),
                start_request(&workflow, 1, None),
            )
            .await,
    );
    assert_eq!(too_deep.code, ControlErrorCode::Invalid);
    assert!(too_deep.message.contains("16"), "{}", too_deep.message);
    // Depth 15 passes the depth check and meets the full share instead.
    let at_15 = code(
        cx.control
            .start(
                &scoped(&tenant, &chain[14], "d15"),
                start_request(&workflow, 1, None),
            )
            .await,
    );
    assert_eq!(at_15.code, ControlErrorCode::Capacity);
    cx.cleanup().await;
}

// ---------------------------------------------------------------------------
// Ownership: cancel before launch, fenced outcomes, parent-close cascade
// ---------------------------------------------------------------------------

impl Children {
    async fn request_id(&self, child: &str) -> Uuid {
        self.engine
            .control_child(&self.fx.tenant, child)
            .await
            .unwrap()
            .unwrap()
            .request_id
    }

    async fn deliver(&self, child: &str) -> Uuid {
        let request = self.request_id(child).await;
        for sql in [
            "UPDATE execution_requests SET state = 'delivered' WHERE request_id = $1",
            "UPDATE execution_outbox SET state = 'delivered' WHERE request_id = $1",
        ] {
            sqlx::query(sql)
                .bind(request)
                .execute(&self.server)
                .await
                .unwrap();
        }
        request
    }

    async fn expire(&self, child: &str) {
        sqlx::query(
            "UPDATE execution_requests SET deadline_at = NOW() - INTERVAL '1 second' WHERE request_id = $1",
        )
        .bind(self.request_id(child).await)
        .execute(&self.server)
        .await
        .unwrap();
        self.engine.outbox().expire_due().await.unwrap();
    }

    /// A control service over a fresh engine: the engine's cached
    /// concurrency count and local admissions start from zero, as after a
    /// restart, instead of counting every admission this test made.
    fn fresh_control(&self) -> NativeControl {
        let (events, _) = tokio::sync::mpsc::channel(64);
        let control = NativeControl::new(Some(self.fx.tenant.clone()));
        control.install(self.runtime.clone());
        control.install_engine(Arc::new(ExecutionEngine::new(
            self.server.clone(),
            Arc::new(WorkflowRepository::new(self.server.clone())),
            Some(self.runtime.clone()),
            None,
            runtara_server::product_events::ProductEventSink::new(events),
        )));
        control
    }

    fn publisher(&self) -> runtara_server::workers::control_children::ControlChildrenPublisher {
        runtara_server::workers::control_children::ControlChildrenPublisher::new(
            self.engine.outbox().clone(),
            self.runtime.clone(),
        )
    }

    async fn published_at(&self, child: &str) -> Option<chrono::DateTime<chrono::Utc>> {
        sqlx::query_scalar(
            "SELECT outcome_published_at FROM execution_requests WHERE tenant_id = $1 AND instance_id = $2",
        )
        .bind(&self.fx.tenant)
        .bind(child)
        .fetch_one(&self.server)
        .await
        .unwrap()
    }
}

/// A child cancelled in admission reads `cancelled` and can never launch; an
/// expired one reads `not-started`; a launch that raced its expiry wins; a
/// cancel stored mid-launch reaches the launched child; `cancel` children
/// still in admission follow their ended parent while `leave_running` ones
/// stay; the passes are idempotent; `query(parent)` lists the outcomes and
/// the public list does not.
#[tokio::test]
async fn children_that_never_launch_get_one_fenced_outcome() {
    let cx = Children::new().await;
    let tenant = cx.fx.tenant.clone();
    let workflow = cx.child_workflow().await;
    let parent = cx.fx.run("parent", &tenant).await;
    cx.fx.park(&parent).await;
    let op = |n: &str| scoped(&tenant, &parent, n);
    let reader = authority(&tenant, None);
    let start = |n: i64, label: &str, policy: ParentClosePolicy| {
        let mut request = start_request(&workflow, n, Some(label));
        request.parent_close_policy = policy;
        request
    };
    let publisher = cx.publisher();

    // Cancelled in admission: applied at once, share freed, fenced.
    let a = cx
        .control
        .start(&op("a"), start(1, "a", ParentClosePolicy::Cancel))
        .await
        .unwrap()
        .instance_id;
    let cancel = |id: &str, reason: &str| CancelRequest {
        instance_id: id.into(),
        reason: Some(reason.into()),
        grace_ms: Some(0),
    };
    let result = cx
        .control
        .cancel(&op("ca"), cancel(&a, "no longer needed"))
        .await
        .unwrap();
    assert_eq!(
        (result.outcome, result.replayed),
        (CommandOutcome::Applied, false)
    );
    assert!(
        cx.control
            .cancel(&op("ca"), cancel(&a, "no longer needed"))
            .await
            .unwrap()
            .replayed
    );
    assert_eq!(
        cx.engine
            .outbox()
            .control_reservations(&tenant)
            .await
            .unwrap(),
        0
    );
    // `get` publishes the outcome inline.
    let read = cx.control.get(&reader, a.clone()).await.unwrap();
    assert_eq!(read.instance.status, InstanceStatus::Cancelled);
    assert_eq!(
        read.instance.termination_reason.as_deref(),
        Some("no longer needed")
    );
    assert_eq!(
        read.instance.parent_instance_id.as_deref(),
        Some(parent.as_str())
    );
    assert!(cx.published_at(&a).await.is_some());
    let fenced = cx
        .fx
        .persistence
        .try_register_child_instance(
            &a,
            &tenant,
            None,
            None,
            &ParentLink {
                parent_instance_id: parent.clone(),
                parent_close_policy: "cancel".into(),
                admitted_at: chrono::Utc::now(),
            },
        )
        .await;
    assert!(fenced.is_err(), "a published outcome refuses the launch");

    // Expired before launch: `not-started`, published by the pass.
    let c = cx
        .control
        .start(&op("c"), start(2, "c", ParentClosePolicy::Cancel))
        .await
        .unwrap()
        .instance_id;
    cx.expire(&c).await;
    let round = publisher.run_once().await.unwrap();
    assert!(round.published >= 1, "{round:?}");
    let first_publication = cx.published_at(&c).await.expect("published");
    let read = cx.control.get(&reader, c.clone()).await.unwrap();
    assert_eq!(read.instance.status, InstanceStatus::NotStarted);
    assert_eq!(
        read.instance.termination_reason.as_deref(),
        Some("execution_outbox_deadline_exceeded")
    );
    publisher.run_once().await.unwrap();
    assert_eq!(
        cx.published_at(&c).await,
        Some(first_publication),
        "the passes are idempotent"
    );

    // Expired, but its launch won the race: the run is the truth.
    let d = cx
        .control
        .start(&op("d"), start(3, "d", ParentClosePolicy::Cancel))
        .await
        .unwrap()
        .instance_id;
    cx.launch(&d).await;
    cx.expire(&d).await;
    let round = publisher.run_once().await.unwrap();
    assert!(round.launched >= 1, "{round:?}");
    assert_eq!(
        cx.control
            .get(&reader, d.clone())
            .await
            .unwrap()
            .instance
            .status,
        InstanceStatus::Pending,
        "no child reads not-started while it runs"
    );
    assert!(
        cx.runtime
            .get_external_outcome(&tenant, &d)
            .await
            .unwrap()
            .is_none()
    );

    // query(parent) lists the outcomes; the public list does not.
    let page = cx
        .control
        .query(
            &reader,
            QueryRequest {
                parent: Some(ParentFilter::Instance(parent.clone())),
                ..query(10)
            },
        )
        .await
        .unwrap();
    let statuses: Vec<_> = page
        .items
        .iter()
        .map(|item| (item.instance_id.as_str(), item.status))
        .collect();
    assert_eq!(
        statuses,
        [
            (a.as_str(), InstanceStatus::Cancelled),
            (c.as_str(), InstanceStatus::NotStarted),
            (d.as_str(), InstanceStatus::Pending),
        ]
    );
    let only_not_started = cx
        .control
        .query(
            &reader,
            QueryRequest {
                parent: Some(ParentFilter::Instance(parent.clone())),
                statuses: vec![InstanceStatus::NotStarted],
                ..query(10)
            },
        )
        .await
        .unwrap();
    assert_eq!(only_not_started.total, 1);
    assert_eq!(only_not_started.items[0].instance_id, c);
    let public = cx
        .engine
        .list_all_executions(
            &tenant,
            None,
            None,
            runtara_server::api::dto::executions::ExecutionFilters {
                parent_instance_id: Some(parent.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        public
            .content
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        [d.as_str()],
        "outcomes stay out of the public list"
    );
    // The launched child ends, so it no longer holds a slot.
    cx.fx.complete(&d, Core::Completed, None, None).await;

    // A cancel stored while launching reaches the child once it launched.
    let b = cx
        .control
        .start(&op("b"), start(4, "b", ParentClosePolicy::LeaveRunning))
        .await
        .unwrap()
        .instance_id;
    let b_request = cx.deliver(&b).await;
    assert_eq!(
        cx.engine
            .outbox()
            .claim_for_launch(b_request, &tenant, &b, "worker")
            .await
            .unwrap(),
        runtara_server::workers::execution_outbox::DurableLaunchClaim::Claimed
    );
    let result = cx
        .control
        .cancel(&op("cb"), cancel(&b, "mid-launch"))
        .await
        .unwrap();
    assert_eq!(result.outcome, CommandOutcome::Requested);
    assert_eq!(
        cx.control
            .get(&reader, b.clone())
            .await
            .unwrap()
            .instance
            .status,
        InstanceStatus::Queued
    );
    cx.launch(&b).await;
    assert!(
        cx.engine
            .outbox()
            .mark_launch_accepted(b_request, "worker")
            .await
            .unwrap()
    );
    cx.fx.park(&b).await;
    let round = publisher.run_once().await.unwrap();
    assert!(round.intents_applied >= 1, "{round:?}");
    assert_eq!(
        cx.control
            .get(&reader, b.clone())
            .await
            .unwrap()
            .instance
            .status,
        InstanceStatus::Cancelled
    );
    publisher.run_once().await.unwrap();
    // The stop was applied once: the intent is marked.
    assert!(cx.published_at(&b).await.is_some());
    cx.engine
        .outbox()
        .release_admission_for_instance(&tenant, &b, "test")
        .await
        .unwrap();

    // The parent ends: its `cancel` child in admission follows, the
    // `leave_running` one stays. (A fresh engine, so the admissions above do
    // not count against the local concurrency gate.)
    let control = cx.fresh_control();
    let e = control
        .start(&op("e"), start(5, "e", ParentClosePolicy::Cancel))
        .await
        .unwrap()
        .instance_id;
    let f = control
        .start(&op("f"), start(6, "f", ParentClosePolicy::LeaveRunning))
        .await
        .unwrap()
        .instance_id;
    let round = publisher.run_once().await.unwrap();
    assert_eq!(round.cascaded, 0, "a suspended parent has not ended");
    cx.fx.complete(&parent, Core::Completed, None, None).await;
    let round = publisher.run_once().await.unwrap();
    assert!(round.cascaded >= 1, "{round:?}");
    let read = cx.control.get(&reader, e.clone()).await.unwrap();
    assert_eq!(read.instance.status, InstanceStatus::Cancelled);
    assert_eq!(
        read.instance.termination_reason,
        Some(format!("parent {parent} terminated (completed)"))
    );
    assert_eq!(
        cx.control
            .get(&reader, f.clone())
            .await
            .unwrap()
            .instance
            .status,
        InstanceStatus::Queued,
        "leave_running survives its parent"
    );
    let round = publisher.run_once().await.unwrap();
    assert_eq!(round.cascaded, 0, "the cascade cancels once");
    cx.cleanup().await;
}

// ---------------------------------------------------------------------------
// Durable instance waits
// ---------------------------------------------------------------------------

use runtara_component_host::control_host::{WaitPoll, WaitResolution};
use runtara_component_host::instance_wait_host::{
    InstanceWaitAuthority, InstanceWaitErrorCode, InstanceWaitHost, InstanceWaitMode,
    InstanceWaitRequest, InstanceWaitResolution, InstanceWaitStatus,
};

fn waiter(tenant: &str, caller: &str) -> InstanceWaitAuthority {
    InstanceWaitAuthority {
        tenant: tenant.into(),
        caller: caller.into(),
    }
}

/// A host-derived wait id, as the workflow host passes it.
fn wait_id(name: &str) -> String {
    format!("{name:0>64}")
}

fn wait_request(ids: &[&String], mode: InstanceWaitMode) -> InstanceWaitRequest {
    InstanceWaitRequest {
        instance_ids: ids.iter().map(|id| (*id).clone()).collect(),
        mode,
        deadline_ms: None,
    }
}

impl Children {
    /// A launched child of `parent`, running.
    async fn launched_child(&self, parent: &str, name: &str) -> String {
        let id = format!("{}-{name}", self.fx.tenant);
        assert!(
            self.fx
                .persistence
                .try_register_child_instance(
                    &id,
                    &self.fx.tenant,
                    None,
                    Some(name),
                    &ParentLink {
                        parent_instance_id: parent.into(),
                        parent_close_policy: "cancel".into(),
                        admitted_at: chrono::Utc::now(),
                    },
                )
                .await
                .unwrap()
        );
        self.fx
            .persistence
            .update_instance_status(&id, Core::Running, None)
            .await
            .unwrap();
        id
    }
}

/// Registering a wait checks, in order: the target cap, then every target
/// (unknown `not-found`, ancestor `denied`, other runs `not-child`, the
/// caller itself `invalid`); nothing registers on a refusal. A replay finds
/// the same wait and keeps its first deadline, other arguments conflict, and
/// a wait id is scoped to its caller.
#[tokio::test]
async fn wait_checks_run_in_order_and_refusals_register_nothing() {
    let cx = Children::new().await;
    let tenant = cx.fx.tenant.clone();
    let grandparent = cx.fx.run("grandparent", &tenant).await;
    let parent = cx.launched_child(&grandparent, "parent").await;
    let child = cx.launched_child(&parent, "child").await;
    let stranger = cx.fx.run("stranger", &tenant).await;
    let waits = cx.control.instance_waits();
    let me = waiter(&tenant, &parent);
    let plain = wait_request(&[&child], InstanceWaitMode::All);

    let too_many: Vec<String> = (0..=1000).map(|i| format!("{tenant}-n{i}")).collect();
    let too_many: Vec<&String> = too_many.iter().collect();
    let missing = format!("{tenant}-missing");
    for (request, code) in [
        (
            wait_request(&too_many, InstanceWaitMode::All),
            InstanceWaitErrorCode::TooLarge,
        ),
        (
            wait_request(&[&child, &missing], InstanceWaitMode::All),
            InstanceWaitErrorCode::NotFound,
        ),
        (
            wait_request(&[&child, &grandparent], InstanceWaitMode::All),
            InstanceWaitErrorCode::Denied,
        ),
        (
            wait_request(&[&child, &stranger], InstanceWaitMode::Any),
            InstanceWaitErrorCode::NotChild,
        ),
        (
            wait_request(&[&child, &parent], InstanceWaitMode::Any),
            InstanceWaitErrorCode::Invalid,
        ),
    ] {
        let refused = wait_id("refused");
        assert_eq!(
            waits
                .register(&me, &refused, request)
                .await
                .unwrap_err()
                .code,
            code
        );
        assert_eq!(
            waits.poll(&me, &refused).await.unwrap_err().code,
            InstanceWaitErrorCode::NotFound,
            "a refused wait registers nothing"
        );
    }
    // Exactly 1000 distinct targets pass the cap (then fail as unknown).
    let at_cap: Vec<String> = (0..1000).map(|i| format!("{tenant}-c{i}")).collect();
    let mut at_cap: Vec<&String> = at_cap.iter().collect();
    at_cap.push(at_cap[0]);
    assert_eq!(
        waits
            .register(
                &me,
                &wait_id("cap"),
                wait_request(&at_cap, InstanceWaitMode::All)
            )
            .await
            .unwrap_err()
            .code,
        InstanceWaitErrorCode::NotFound,
        "duplicates count once"
    );

    let registered = wait_id("w");
    let pending = waits
        .register(&me, &registered, plain.clone())
        .await
        .unwrap();
    assert!(!pending.is_settled());
    assert_eq!(pending.remaining, std::slice::from_ref(&child));
    assert_eq!(
        waits
            .register(&me, &registered, plain.clone())
            .await
            .unwrap(),
        pending,
        "a replay finds the same wait"
    );
    let mut later = plain.clone();
    later.deadline_ms = Some(4_102_444_800_000);
    assert_eq!(
        waits
            .register(&me, &registered, later)
            .await
            .unwrap()
            .deadline_ms,
        None,
        "the first deadline stands"
    );
    assert_eq!(
        waits
            .register(
                &me,
                &registered,
                wait_request(&[&child], InstanceWaitMode::Any)
            )
            .await
            .unwrap_err()
            .code,
        InstanceWaitErrorCode::ReplayConflict
    );
    assert_eq!(
        waits
            .poll(&waiter(&tenant, &grandparent), &registered)
            .await
            .unwrap_err()
            .code,
        InstanceWaitErrorCode::NotFound,
        "another run's wait under the same id is another wait"
    );
    cx.cleanup().await;
}

/// A wait over children in every state: a never-launched child reads
/// `not-started` (its outcome is published first), a child still in
/// admission stays remaining, launched ones finish with their results under
/// the per-target and 3 MiB caps, the same values on every read; and a
/// target whose rows disappear is an explicit `not-found`.
#[tokio::test]
async fn wait_reports_children_in_every_state_under_stable_caps() {
    let cx = Children::new().await;
    let tenant = cx.fx.tenant.clone();
    let workflow = cx.child_workflow().await;
    let parent = cx.fx.run("parent", &tenant).await;
    cx.fx
        .persistence
        .update_instance_status(&parent, Core::Running, None)
        .await
        .unwrap();
    let op = |n: &str| scoped(&tenant, &parent, n);
    let control = cx.fresh_control();
    let waits = control.instance_waits();
    let me = waiter(&tenant, &parent);
    let expired = control
        .start(&op("s1"), start_request(&workflow, 1, Some("expired")))
        .await
        .unwrap()
        .instance_id;
    let queued = control
        .start(&op("s2"), start_request(&workflow, 2, Some("queued")))
        .await
        .unwrap()
        .instance_id;
    cx.expire(&expired).await;
    assert!(cx.published_at(&expired).await.is_none());
    // Twelve launched children with 256 KiB outputs (the per-target cap),
    // one over it and one that fails.
    let mut launched = Vec::new();
    for i in 0..12 {
        launched.push(cx.launched_child(&parent, &format!("big-{i}")).await);
    }
    let huge = cx.launched_child(&parent, "huge").await;
    let failed = cx.launched_child(&parent, "failed").await;
    let mut targets: Vec<&String> = launched.iter().collect();
    targets.extend([&huge, &failed, &expired, &queued]);

    let id = wait_id("wait");
    let pending = waits
        .register(&me, &id, wait_request(&targets, InstanceWaitMode::All))
        .await
        .unwrap();
    assert!(
        cx.published_at(&expired).await.is_some(),
        "the ended admission is published under the fence first"
    );
    assert!(!pending.is_settled(), "{pending:?}");
    assert_eq!(pending.finished.len(), 1);
    assert_eq!(pending.finished[0].instance_id, expired);
    assert_eq!(pending.finished[0].status, InstanceWaitStatus::NotStarted);
    assert!(pending.remaining.contains(&queued));

    let output = |i: usize| {
        let mut bytes = format!("{{\"i\":{i},\"pad\":\"").into_bytes();
        bytes.resize(256 * 1024 - 2, b'x');
        bytes.extend_from_slice(b"\"}");
        bytes
    };
    cx.fx
        .complete(&failed, Core::Failed, None, Some("boom"))
        .await;
    for (i, child) in launched.iter().enumerate() {
        cx.fx
            .complete(child, Core::Completed, Some(&output(i)), None)
            .await;
    }
    let huge_output = format!("\"{}\"", "h".repeat(256 * 1024));
    cx.fx
        .complete(&huge, Core::Completed, Some(huge_output.as_bytes()), None)
        .await;
    // The queued child ends too: its admission expires, and the read
    // publishes its outcome.
    cx.expire(&queued).await;
    let first = waits.poll(&me, &id).await.unwrap();
    assert_eq!(first.resolution, Some(InstanceWaitResolution::Satisfied));
    assert_eq!(first.finished.len(), targets.len());
    assert!(first.remaining.is_empty());
    let by_id = |target_id: &String| {
        first
            .finished
            .iter()
            .find(|target| &target.instance_id == target_id)
            .unwrap()
    };
    assert_eq!(by_id(&queued).status, InstanceWaitStatus::NotStarted);
    let huge_read = by_id(&huge);
    assert!(huge_read.output.is_none() && huge_read.output_omitted);
    assert_eq!(huge_read.output_bytes, Some(huge_output.len() as u64));
    assert_eq!(
        by_id(&failed).error.as_deref(),
        Some(br#""boom""#.as_slice())
    );
    let inlined: usize = first
        .finished
        .iter()
        .filter_map(|target| target.output.as_ref().map(Vec::len))
        .sum();
    assert!(inlined <= 3 * 1024 * 1024, "the 3 MiB budget holds");
    let omitted = launched
        .iter()
        .filter(|child| by_id(child).output_omitted)
        .count();
    assert_eq!(
        omitted, 1,
        "the error and 11 x 256 KiB fill the budget in finish order; the 12th is omitted"
    );
    // Reads are stable: the same selection, the same inlined values.
    let second = waits.poll(&me, &id).await.unwrap();
    assert_eq!(first, second, "the budget is spent the same way every read");

    // A finished target whose rows disappear is an explicit error.
    sqlx::query("DELETE FROM instances WHERE instance_id = $1")
        .bind(&failed)
        .execute(&cx.fx.pool)
        .await
        .unwrap();
    let gone = waits.poll(&me, &id).await.unwrap_err();
    assert_eq!(gone.code, InstanceWaitErrorCode::NotFound);
    assert!(gone.message.contains(&failed), "{}", gone.message);
    cx.cleanup().await;
}

/// The `InstanceWaitHost` boundary the workflow host calls: `register`
/// authorizes and registers, then returns the evaluated wait; a replay finds
/// the same wait and keeps its first deadline; the wait id is scoped to the
/// caller; and the control adapter reads the very same wait.
#[tokio::test]
async fn instance_waits_register_and_evaluate_through_the_host_trait() {
    let cx = Children::new().await;
    let tenant = cx.fx.tenant.clone();
    let grandparent = cx.fx.run("grandparent", &tenant).await;
    let parent = cx.launched_child(&grandparent, "parent").await;
    let first = cx.launched_child(&parent, "first").await;
    let second = cx.launched_child(&parent, "second").await;
    let waits = cx.control.instance_waits();
    let me = InstanceWaitAuthority {
        tenant: tenant.clone(),
        caller: parent.clone(),
    };
    let wait_id = format!("{:0>64}", "step");
    let request = |ids: &[&String], deadline_ms| InstanceWaitRequest {
        instance_ids: ids.iter().map(|id| (*id).clone()).collect(),
        mode: InstanceWaitMode::Any,
        deadline_ms,
    };

    // D1 before anything registers.
    let refused = waits
        .register(&me, &wait_id, request(&[&first, &grandparent], None))
        .await
        .unwrap_err();
    assert_eq!(refused.code, InstanceWaitErrorCode::Denied);
    assert_eq!(
        waits.poll(&me, &wait_id).await.unwrap_err().code,
        InstanceWaitErrorCode::NotFound,
        "a refused wait registers nothing"
    );

    let deadline = 4_102_444_800_000;
    let pending = waits
        .register(&me, &wait_id, request(&[&second, &first], Some(deadline)))
        .await
        .unwrap();
    assert!(!pending.is_settled());
    assert_eq!(pending.mode, InstanceWaitMode::Any);
    assert!(pending.finished.is_empty());
    assert_eq!(pending.deadline_ms, Some(deadline));
    let mut sorted = vec![first.clone(), second.clone()];
    sorted.sort();
    assert_eq!(pending.remaining, sorted);
    // Another caller's wait under the same id is another wait.
    let stranger = InstanceWaitAuthority {
        tenant: tenant.clone(),
        caller: grandparent.clone(),
    };
    assert_eq!(
        waits.poll(&stranger, &wait_id).await.unwrap_err().code,
        InstanceWaitErrorCode::NotFound
    );

    cx.fx
        .complete(&first, Core::Completed, Some(br#"{"approved":true}"#), None)
        .await;
    // A replay (later deadline, reordered ids) finds the settled wait.
    let settled = waits
        .register(&me, &wait_id, request(&[&first, &second], None))
        .await
        .unwrap();
    assert_eq!(settled.resolution, Some(InstanceWaitResolution::Satisfied));
    assert_eq!(
        settled.deadline_ms,
        Some(deadline),
        "the first deadline stands"
    );
    assert_eq!(settled.finished.len(), 1);
    assert_eq!(settled.finished[0].instance_id, first);
    assert_eq!(settled.finished[0].status, InstanceWaitStatus::Completed);
    assert_eq!(
        serde_json::to_value(&settled).unwrap()["finished"][0]["output"],
        json!({"approved": true})
    );
    assert_eq!(settled.remaining, vec![second.clone()]);
    assert_eq!(waits.poll(&me, &wait_id).await.unwrap(), settled);
    let conflict = waits
        .register(
            &me,
            &wait_id,
            InstanceWaitRequest {
                mode: InstanceWaitMode::All,
                ..request(&[&first, &second], None)
            },
        )
        .await
        .unwrap_err();
    assert_eq!(conflict.code, InstanceWaitErrorCode::ReplayConflict);

    // Control's `poll-wait` is an adapter over the same wait.
    let via_control = cx
        .control
        .poll_wait(&scoped(&tenant, &parent, "step"), wait_id.clone())
        .await
        .unwrap();
    let WaitPoll::Settled(control_settled) = via_control else {
        panic!("{via_control:?}")
    };
    assert_eq!(control_settled.resolution, WaitResolution::Satisfied);
    assert_eq!(control_settled.progress.finished[0].instance_id, first);
    assert_eq!(control_settled.progress.remaining, vec![second]);
    cx.cleanup().await;
}
