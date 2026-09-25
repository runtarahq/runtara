//! Report receipt replay over separate, isolated server and runtime databases.
use runtara_connections::{
    ConnectionsConfig, ConnectionsFacade, ConnectionsState, IntegrationCompatibility,
    crypto::noop::NoOpCipher,
};
use runtara_core::{
    domain::InstanceStatus,
    persistence::{
        Persistence,
        inputs::{InputAuthority, InputError, InputRequestSpec, InputState, submit_input},
    },
};
use runtara_environment::{handlers::EnvironmentHandlerState, runner::MockRunner};
use runtara_server::{
    api::{
        dto::reports::{ReportDto, SubmitReportWorkflowActionRequest},
        repositories::{
            object_model::ObjectStoreManager, reports::ReportRepository,
            workflows::WorkflowRepository,
        },
        services::{
            reports::{ReportService, ReportServiceError},
            workflow_runtime::WorkflowRuntimeError,
        },
    },
    auth::{AuthContext, AuthMethod},
    product_events::ProductEventSink,
    runtime_client::{RuntimeClient, RuntimeClientConfig},
    workers::execution_engine::ExecutionEngine,
};
use runtara_store_postgres::PostgresPersistence;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
struct ReportState {
    pool: PgPool,
    manager: Arc<ObjectStoreManager>,
    connections: Arc<ConnectionsFacade>,
    engine: Arc<ExecutionEngine>,
    client: Option<Arc<RuntimeClient>>,
}

#[cfg(feature = "valkey-integration-tests")]
macro_rules! report_state_field {
    ($ty:ty, $field:ident) => {
        impl axum::extract::FromRef<ReportState> for $ty {
            fn from_ref(state: &ReportState) -> Self {
                state.$field.clone()
            }
        }
    };
}
#[cfg(feature = "valkey-integration-tests")]
report_state_field!(PgPool, pool);
#[cfg(feature = "valkey-integration-tests")]
report_state_field!(Arc<ObjectStoreManager>, manager);
#[cfg(feature = "valkey-integration-tests")]
report_state_field!(Arc<ConnectionsFacade>, connections);
#[cfg(feature = "valkey-integration-tests")]
report_state_field!(Arc<ExecutionEngine>, engine);
#[cfg(feature = "valkey-integration-tests")]
report_state_field!(Option<Arc<RuntimeClient>>, client);

struct Fixture {
    server: PgPool,
    persistence: Arc<PostgresPersistence>,
    service: ReportService,
    tenant: String,
    instance: String,
    request: String,
    report: ReportDto,
    auth: AuthContext,
    #[cfg(feature = "valkey-integration-tests")]
    http_state: ReportState,
}

impl Fixture {
    async fn new() -> Self {
        let runtime = PgPool::connect(
            &std::env::var("TEST_ENVIRONMENT_DATABASE_URL")
                .or_else(|_| std::env::var("TEST_RUNTARA_DATABASE_URL"))
                .expect("isolated runtime test database required"),
        )
        .await
        .expect("connect runtime test database");
        let server = PgPool::connect(
            &std::env::var("TEST_RUNTARA_SERVER_DATABASE_URL")
                .expect("isolated server test database required"),
        )
        .await
        .expect("connect server test database");
        runtara_environment::migrations::run(&runtime)
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&server).await.unwrap();
        let persistence = Arc::new(PostgresPersistence::new(runtime.clone()));
        let client = Arc::new(RuntimeClient::new(
            Arc::new(EnvironmentHandlerState::new(
                runtime.clone(),
                persistence.clone(),
                Arc::new(MockRunner::new()),
                std::env::temp_dir(),
            )),
            RuntimeClientConfig::new(Default::default()),
        ));
        let (events, _receiver) = tokio::sync::mpsc::channel(1);
        let engine = Arc::new(ExecutionEngine::new(
            server.clone(),
            Arc::new(WorkflowRepository::new(server.clone())),
            Some(client.clone()),
            None,
            ProductEventSink::new(events),
        ));
        let connections = Arc::new(ConnectionsFacade::new(ConnectionsState::from_config(
            ConnectionsConfig {
                db_pool: server.clone(),
                redis_manager: None,
                public_base_url: "http://localhost".into(),
                http_client: reqwest::Client::new(),
                cipher: Arc::new(NoOpCipher),
                compatibility: Arc::new(IntegrationCompatibility::default()),
                agent_catalog: Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![])),
                connection_events: None,
            },
        )));
        let http_state = ReportState {
            pool: server.clone(),
            manager: Arc::new(ObjectStoreManager::new(
                "postgresql://unused.invalid/unused".into(),
            )),
            connections,
            engine,
            client: Some(client),
        };
        let service = ReportService::new(
            http_state.pool.clone(),
            http_state.manager.clone(),
            http_state.connections.clone(),
        )
        .with_runtime(http_state.engine.clone(), http_state.client.clone());
        let tenant = Uuid::new_v4().to_string();
        let workflow = Uuid::new_v4().to_string();
        let instance = Uuid::new_v4().to_string();
        let image = Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO images (image_id,tenant_id,name,binary_path) VALUES ($1,$2,$3,'/test-only')")
            .bind(&image).bind(&tenant).bind(format!("{workflow}:1"))
            .execute(&runtime).await.unwrap();
        persistence
            .register_instance(&instance, &tenant)
            .await
            .unwrap();
        persistence
            .update_instance_status(&instance, InstanceStatus::Running, None)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO instance_images (instance_id,image_id,tenant_id) VALUES ($1,$2,$3)",
        )
        .bind(&instance)
        .bind(&image)
        .bind(&tenant)
        .execute(&runtime)
        .await
        .unwrap();
        let registered = persistence
            .input_requests()
            .unwrap()
            .register_input(
                &InputAuthority::Root {
                    tenant_id: tenant.clone(),
                    instance_id: instance.clone(),
                },
                &InputRequestSpec {
                    signal_id: "report-approval".into(),
                    response_schema: Some(json!({
                        "answer":{"type":"boolean","required":true},
                        "viewer":{"type":"string","required":true},
                        "decision":{"type":"boolean","required":true}
                    })),
                    metadata: json!({"step_name":"Approve"}),
                    deadline: None,
                },
            )
            .await
            .unwrap();
        let report: ReportDto = serde_json::from_value(json!({
            "id":Uuid::new_v4().to_string(), "slug":format!("approval-{tenant}"),
            "name":"Approval", "status":"published", "definitionVersion":1,
            "createdAt":chrono::Utc::now(), "updatedAt":chrono::Utc::now(),
            "definition":{"definitionVersion":1,"blocks":[{
                "id":"approve","type":"actions",
                "source":{"kind":"workflow_runtime","entity":"actions","workflowId":workflow},
                "actions":{"submit":{"implicitPayload":{
                    "viewer":"{{viewer.user_id}}","decision":true
                }}}
            }]}
        }))
        .unwrap();
        let auth = AuthContext::new(tenant.clone(), "report-viewer".into(), AuthMethod::Jwt);
        let report = ReportRepository::new(server.clone())
            .create(&tenant, &report, Some(&auth.user_id))
            .await
            .unwrap();
        Self {
            server,
            persistence,
            service,
            tenant,
            instance,
            request: registered.request_id,
            report,
            auth,
            #[cfg(feature = "valkey-integration-tests")]
            http_state,
        }
    }

    fn submission(&self) -> SubmitReportWorkflowActionRequest {
        SubmitReportWorkflowActionRequest {
            instance_id: self.instance.clone(),
            request_id: self.request.clone(),
            operation_id: "report-response".into(),
            payload: json!({"answer":true}),
            filters: Default::default(),
            block_filters: Default::default(),
        }
    }

    async fn submit(
        &self,
        request: SubmitReportWorkflowActionRequest,
        auth: &AuthContext,
    ) -> Result<Value, ReportServiceError> {
        self.service
            .submit_report_workflow_action(
                &self.tenant,
                &self.report.slug,
                "approve",
                &self.request,
                request,
                auth,
            )
            .await
    }

    async fn save_report(&self) {
        ReportRepository::new(self.server.clone())
            .update(&self.tenant, &self.report.id, &self.report)
            .await
            .unwrap()
            .unwrap();
    }

    async fn state(&self) -> InputState {
        self.persistence
            .input_requests()
            .unwrap()
            .get_input(&self.tenant, &self.instance, &self.request)
            .await
            .unwrap()
            .state
    }
}

fn assert_conflict(result: Result<Value, ReportServiceError>) {
    assert!(
        matches!(
            result,
            Err(ReportServiceError::WorkflowRuntime(
                WorkflowRuntimeError::Managed(InputError::OperationConflict)
            ))
        ),
        "{result:?}"
    );
}

#[tokio::test]
async fn report_replay_preserves_effective_payload_after_defaults_and_liveness_change() {
    let mut f = Fixture::new().await;
    let first = f.submit(f.submission(), &f.auth).await.unwrap();
    assert_eq!(first.as_object().unwrap().len(), 3);
    for key in ["receiptId", "requestId", "acceptedAt"] {
        assert!(first.get(key).is_some());
    }
    let original = f.state().await;
    let InputState::Accepted { receipt } = &original else {
        panic!("accepted receipt required")
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&receipt.payload).unwrap(),
        json!({"answer":true,"viewer":"report-viewer","decision":true})
    );
    assert!(receipt.acceptance_context.is_some());

    f.report.slug.push_str("-renamed");
    f.report.definition.blocks[0]
        .actions
        .as_mut()
        .unwrap()
        .submit
        .as_mut()
        .unwrap()
        .implicit_payload
        .insert(
            "decision".into(),
            json!("now invalid for the registered schema"),
        );
    f.save_report().await;
    f.persistence
        .update_instance_status(&f.instance, InstanceStatus::Completed, None)
        .await
        .unwrap();
    assert_eq!(f.submit(f.submission(), &f.auth).await.unwrap(), first);
    assert_eq!(f.state().await, original);

    let mut changed = f.submission();
    changed.payload = json!({"answer":false});
    assert_conflict(f.submit(changed, &f.auth).await);
    let mut changed = f.submission();
    changed.filters.insert("selection".into(), json!("another"));
    assert_conflict(f.submit(changed, &f.auth).await);
    let mut changed = f.submission();
    changed
        .block_filters
        .insert("selection".into(), json!("another"));
    assert_conflict(f.submit(changed, &f.auth).await);
    let other = AuthContext::new(f.tenant.clone(), "another-viewer".into(), AuthMethod::Jwt);
    assert_conflict(f.submit(f.submission(), &other).await);
    let other_method =
        AuthContext::new(f.tenant.clone(), f.auth.user_id.clone(), AuthMethod::ApiKey);
    assert_conflict(f.submit(f.submission(), &other_method).await);
    assert_eq!(
        submit_input(
            f.persistence.input_requests().unwrap(),
            &f.tenant,
            &f.instance,
            &f.request,
            "report-response",
            &serde_json::from_slice::<Value>(&receipt.payload).unwrap()
        )
        .await,
        Err(InputError::OperationConflict)
    );
    assert_eq!(f.state().await, original);

    // A second authorized block/report cannot borrow the first one's operation.
    let mut block = f.report.definition.blocks[0].clone();
    block.id = "second-block".into();
    f.report.definition.blocks.push(block);
    f.save_report().await;
    assert_conflict(
        f.service
            .submit_report_workflow_action(
                &f.tenant,
                &f.report.id,
                "second-block",
                &f.request,
                f.submission(),
                &f.auth,
            )
            .await,
    );
    let mut other_report = f.report.clone();
    other_report.id = Uuid::new_v4().to_string();
    other_report.slug.push_str("-second");
    ReportRepository::new(f.server.clone())
        .create(&f.tenant, &other_report, Some(&f.auth.user_id))
        .await
        .unwrap();
    assert_conflict(
        f.service
            .submit_report_workflow_action(
                &f.tenant,
                &other_report.id,
                "approve",
                &f.request,
                f.submission(),
                &f.auth,
            )
            .await,
    );
}

#[tokio::test]
async fn report_replay_reauthorizes_current_scope_and_report_existence() {
    let mut f = Fixture::new().await;
    let first = f.submit(f.submission(), &f.auth).await.unwrap();
    let foreign = AuthContext::new("foreign".into(), f.auth.user_id.clone(), AuthMethod::Jwt);
    assert!(matches!(
        f.submit(f.submission(), &foreign).await,
        Err(ReportServiceError::NotFound)
    ));
    f.report.definition.blocks[0].source.instance_id = Some("other-instance".into());
    f.save_report().await;
    assert!(matches!(
        f.submit(f.submission(), &f.auth).await,
        Err(ReportServiceError::WorkflowRuntime(
            WorkflowRuntimeError::Managed(InputError::NotFound)
        ))
    ));
    f.report.definition.blocks[0].source.instance_id = None;
    f.save_report().await;
    assert_eq!(f.submit(f.submission(), &f.auth).await.unwrap(), first);
    f.report.definition.blocks[0].source.condition = Some(
        serde_json::from_value(json!({"op":"EQ","arguments":["requestId","another-request"]}))
            .unwrap(),
    );
    f.save_report().await;
    assert!(matches!(
        f.submit(f.submission(), &f.auth).await,
        Err(ReportServiceError::WorkflowRuntime(
            WorkflowRuntimeError::Managed(InputError::NotFound)
        ))
    ));
    f.report.definition.blocks[0].source.condition = None;
    let workflow = f.report.definition.blocks[0]
        .source
        .workflow_id
        .replace("another-workflow".into());
    f.save_report().await;
    assert!(matches!(
        f.submit(f.submission(), &f.auth).await,
        Err(ReportServiceError::WorkflowRuntime(
            WorkflowRuntimeError::NotFound(_)
        ))
    ));
    f.report.definition.blocks[0].source.workflow_id = workflow;
    f.save_report().await;
    assert_eq!(f.submit(f.submission(), &f.auth).await.unwrap(), first);
    ReportRepository::new(f.server.clone())
        .delete(&f.tenant, &f.report.id)
        .await
        .unwrap();
    assert!(matches!(
        f.submit(f.submission(), &f.auth).await,
        Err(ReportServiceError::NotFound)
    ));
}

#[tokio::test]
async fn report_authorization_storage_failure_cannot_replay_a_receipt() {
    let f = Fixture::new().await;
    f.submit(f.submission(), &f.auth).await.unwrap();
    let accepted = f.state().await;
    // Close only this fixture's server pool; the runtime receipt remains readable.
    f.server.close().await;
    assert!(matches!(
        f.submit(f.submission(), &f.auth).await,
        Err(ReportServiceError::Database(_))
    ));
    assert_eq!(f.state().await, accepted);
}

#[tokio::test]
async fn invalid_new_report_response_leaves_request_open_for_corrected_submission() {
    let f = Fixture::new().await;
    let mut invalid = f.submission();
    invalid.payload = json!({"answer":"invalid"});
    assert!(matches!(
        f.submit(invalid, &f.auth).await,
        Err(ReportServiceError::WorkflowRuntime(
            WorkflowRuntimeError::Managed(InputError::InvalidPayload(_))
        ))
    ));
    assert_eq!(f.state().await, InputState::Open);
    let first = f.submit(f.submission(), &f.auth).await.unwrap();
    assert_eq!(f.submit(f.submission(), &f.auth).await.unwrap(), first);
}

#[cfg(feature = "valkey-integration-tests")]
mod http_authorization {
    use super::*;
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{HeaderMap, Request, StatusCode},
        middleware::{from_fn, from_fn_with_state},
        routing::post,
    };
    use redis::AsyncCommands;
    use runtara_server::{
        auth::{AuthError, AuthProvider, AuthProviderKind, AuthState, MembershipPolicy},
        middleware::{auth::authenticate, authorization::authorize},
    };
    use sha2::Digest;
    use tower::ServiceExt;

    // Signature verification is outside this test; membership/revocation, API-key
    // validation, route authorization, extractors and the handler are production code.
    struct VerifiedIdentity(AuthContext);

    #[async_trait::async_trait]
    impl AuthProvider for VerifiedIdentity {
        async fn authenticate(&self, _headers: &HeaderMap) -> Result<AuthContext, AuthError> {
            Ok(self.0.clone())
        }
        fn kind(&self) -> AuthProviderKind {
            AuthProviderKind::Oidc
        }
    }

    async fn post_submission(app: &Router, f: &Fixture, key: Option<&str>) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method("POST")
            .uri(format!(
                "/api/runtime/reports/{}/blocks/approve/actions/{}/submit",
                f.report.id, f.request,
            ))
            .header("content-type", "application/json");
        if let Some(key) = key {
            request = request.header("authorization", format!("Bearer {key}"));
        }
        let response = app
            .clone()
            .oneshot(
                request
                    .body(Body::from(serde_json::to_vec(&f.submission()).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    #[tokio::test]
    async fn retained_report_receipts_require_live_membership_and_unrevoked_credentials() {
        let config =
            runtara_server::valkey::ValkeyConfig::from_env().expect("isolated Valkey required");
        let mut conn = redis::aio::ConnectionManager::new(
            runtara_server::valkey::open_client(&config.connection_url()).unwrap(),
        )
        .await
        .unwrap();
        for api_key in [false, true] {
            let mut f = Fixture::new().await;
            f.auth.user_id = Uuid::new_v4().to_string();
            let jti = Uuid::new_v4().to_string();
            f.auth.jti = Some(jti.clone());
            let member_key = format!("member:{}", f.auth.user_id);
            let revoked_key = format!("token:revoked:{jti}");
            let _: () = conn.set(&member_key, r#"{"role":"member"}"#).await.unwrap();
            // Synthetic, disposable credential; only its hash goes into the fixture DB.
            let key = api_key.then(|| format!("rt_{}", Uuid::new_v4().simple()));
            if let Some(key) = &key {
                sqlx::query("INSERT INTO api_keys (org_id,name,key_prefix,key_hash,created_by,issuing_user_id,jti) VALUES ($1,'report retry test','rt_test',$2,$3,$3,$4)")
                    .bind(&f.tenant).bind(hex::encode(sha2::Sha256::digest(key.as_bytes())))
                    .bind(&f.auth.user_id).bind(&jti).execute(&f.server).await.unwrap();
            }
            let auth = AuthState {
                provider: Arc::new(VerifiedIdentity(f.auth.clone())),
                pool: f.server.clone(),
                valkey: Some(conn.clone()),
                membership_policy: MembershipPolicy::Required,
            };
            let app = Router::new()
                .route(
                    "/api/runtime/reports/{report_id}/blocks/{block_id}/actions/{action_id}/submit",
                    post(runtara_server::api::handlers::reports::submit_report_workflow_action),
                )
                .route_layer(from_fn(authorize(MembershipPolicy::Required)))
                .layer(from_fn_with_state(auth, authenticate))
                .with_state(f.http_state.clone());
            let (status, first) = post_submission(&app, &f, key.as_deref()).await;
            assert_eq!(status, StatusCode::OK, "{first}");
            f.persistence
                .update_instance_status(&f.instance, InstanceStatus::Completed, None)
                .await
                .unwrap();
            assert_eq!(
                post_submission(&app, &f, key.as_deref()).await,
                (StatusCode::OK, first.clone())
            );

            let _: () = conn.del(&member_key).await.unwrap();
            let (status, denial) = post_submission(&app, &f, key.as_deref()).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{denial}");
            assert!(denial.get("receiptId").is_none());
            let _: () = conn.set(&member_key, r#"{"role":"viewer"}"#).await.unwrap();
            // Report consumption deliberately permits viewers; membership removal
            // revokes access, while a valid viewer role still permits this receipt.
            assert_eq!(
                post_submission(&app, &f, key.as_deref()).await,
                (StatusCode::OK, first)
            );
            let _: () = conn.set(&revoked_key, "revoked").await.unwrap();
            let (status, denial) = post_submission(&app, &f, key.as_deref()).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{denial}");
            assert!(denial.get("receiptId").is_none());
            let _: () = conn.del(&revoked_key).await.unwrap();
            if api_key {
                sqlx::query("UPDATE api_keys SET is_revoked=true WHERE org_id=$1 AND jti=$2")
                    .bind(&f.tenant)
                    .bind(&jti)
                    .execute(&f.server)
                    .await
                    .unwrap();
                assert_eq!(
                    post_submission(&app, &f, key.as_deref()).await.0,
                    StatusCode::UNAUTHORIZED
                );
            }
            let _: () = conn.del(&member_key).await.unwrap();
        }
    }
}
