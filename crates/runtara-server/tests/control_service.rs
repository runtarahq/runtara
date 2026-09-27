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
