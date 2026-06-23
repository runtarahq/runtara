//! Reports runtime corpus tests — Phase 0 of the reports refactor.
//!
//! Runs every fixture through `ReportService::validate_report`, including
//! semantic checks (schema/workflow lookups). Snapshots the response so any
//! drift surfaces in `cargo insta review` during later phases.
//!
//! Requires a running Postgres with `pgvector` and `pg_trgm` extensions.
//! Reads `TEST_REPORTS_DATABASE_URL` or falls back to `RUNTARA_DATABASE_URL`
//! / `RUNTARA_SERVER_DATABASE_URL`. Skips gracefully when none is set or unreachable.
//!
//! The test creates a UUID-suffixed throwaway database for each run, applies
//! server migrations, runs the corpus, and drops the database on success.
//! On failure the database is left intact for debugging.

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use runtara_connections::crypto::noop::NoOpCipher;
use runtara_connections::{
    ConnectionsConfig, ConnectionsFacade, ConnectionsState, IntegrationCompatibility,
};
use runtara_server::api::dto::reports::ReportDefinition;
use runtara_server::api::repositories::object_model::ObjectStoreManager;
use runtara_server::api::services::reports::ReportService;
use runtara_server::config::Config;
use serde_json::json;
use sqlx::PgPool;
use sqlx::postgres::PgConnectOptions;
use std::str::FromStr;
use uuid::Uuid;

static CONFIG_INIT: OnceLock<()> = OnceLock::new();

fn ensure_config(object_url: &str) {
    CONFIG_INIT.get_or_init(|| {
        // Required by `Config::from_env`; only consulted on first init.
        unsafe {
            std::env::set_var("TENANT_ID", TENANT_ID);
            std::env::set_var("OBJECT_MODEL_DATABASE_URL", object_url);
        }
        let config = Config::from_env().expect("build test Config");
        runtara_server::config::init(config);
    });
}

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

const TENANT_ID: &str = "tenant_reports_corpus";

fn base_database_url() -> Option<String> {
    let _ = dotenvy::from_path(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.env"));
    for var in [
        "TEST_REPORTS_DATABASE_URL",
        "RUNTARA_DATABASE_URL",
        "RUNTARA_SERVER_DATABASE_URL",
    ] {
        if let Ok(value) = std::env::var(var)
            && !value.is_empty()
        {
            return Some(value);
        }
    }
    None
}

struct DbFixture {
    server_pool: PgPool,
    object_url: String,
    admin_url: String,
    server_db: String,
    object_db: String,
}

impl DbFixture {
    async fn start() -> Option<Self> {
        let base = base_database_url()?;
        let opts = match PgConnectOptions::from_str(&base) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("invalid base database url: {e}");
                return None;
            }
        };
        let host = opts.get_host().to_string();
        let port = opts.get_port();
        let user = opts.get_username().to_string();
        let password = url::Url::parse(&base)
            .ok()
            .and_then(|u| u.password().map(str::to_owned))
            .unwrap_or_default();
        let admin_db = opts.get_database().unwrap_or("postgres").to_string();
        let auth = if password.is_empty() {
            user.clone()
        } else {
            format!("{user}:{password}")
        };

        let suffix = Uuid::new_v4().simple().to_string();
        let server_db = format!("runtara_reports_corpus_{suffix}");
        let object_db = format!("runtara_reports_corpus_object_{suffix}");

        let admin_url = format!("postgres://{auth}@{host}:{port}/{admin_db}");
        let admin_pool = match PgPool::connect(&admin_url).await {
            Ok(p) => p,
            Err(e) => {
                eprintln!("connect to admin Postgres ({admin_url}): {e}");
                return None;
            }
        };

        if let Err(e) = sqlx::query(&format!("CREATE DATABASE \"{server_db}\""))
            .execute(&admin_pool)
            .await
        {
            eprintln!("create server db: {e}");
            return None;
        }
        if let Err(e) = sqlx::query(&format!("CREATE DATABASE \"{object_db}\""))
            .execute(&admin_pool)
            .await
        {
            let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS \"{server_db}\""))
                .execute(&admin_pool)
                .await;
            eprintln!("create object db: {e}");
            return None;
        }
        admin_pool.close().await;

        let server_url = format!("postgres://{auth}@{host}:{port}/{server_db}");
        let object_url = format!("postgres://{auth}@{host}:{port}/{object_db}");

        let server_pool = match PgPool::connect(&server_url).await {
            Ok(p) => p,
            Err(e) => {
                eprintln!("connect to server db: {e}");
                return None;
            }
        };
        if let Err(e) = MIGRATOR.run(&server_pool).await {
            eprintln!("apply server migrations: {e}");
            return None;
        }

        Some(Self {
            server_pool,
            object_url,
            admin_url,
            server_db,
            object_db,
        })
    }

    async fn cleanup(self) {
        let DbFixture {
            server_pool,
            admin_url,
            server_db,
            object_db,
            ..
        } = self;
        server_pool.close().await;
        if let Ok(admin) = PgPool::connect(&admin_url).await {
            let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS \"{server_db}\""))
                .execute(&admin)
                .await;
            let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS \"{object_db}\""))
                .execute(&admin)
                .await;
            admin.close().await;
        }
    }
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/reports")
}

fn collect_fixtures() -> Vec<(String, ReportDefinition)> {
    let dir = fixtures_dir();
    let mut entries: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    entries.sort();
    entries
        .into_iter()
        .map(|path| {
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_owned)
                .unwrap_or_else(|| path.display().to_string());
            let raw = fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            let definition: ReportDefinition =
                serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{name}: parse: {e}"));
            (name, definition)
        })
        .collect()
}

#[tokio::test]
async fn validate_report_snapshots() {
    let Some(fixture) = DbFixture::start().await else {
        eprintln!(
            "Skipping reports runtime corpus: set TEST_REPORTS_DATABASE_URL or RUNTARA_DATABASE_URL"
        );
        return;
    };

    ensure_config(&fixture.object_url);

    let connections = Arc::new(ConnectionsFacade::new(ConnectionsState::from_config(
        ConnectionsConfig {
            db_pool: fixture.server_pool.clone(),
            redis_manager: None,
            public_base_url: "http://localhost".to_string(),
            http_client: reqwest::Client::new(),
            cipher: Arc::new(NoOpCipher),
            compatibility: Arc::new(IntegrationCompatibility::default()),
            agent_catalog: Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(
                Vec::new(),
            )),
        },
    )));

    connections
        .ensure_default_connection(
            TENANT_ID,
            "object_model",
            "Default Object Model DB".to_string(),
            "postgres".to_string(),
            json!({ "database_url": fixture.object_url }),
        )
        .await
        .expect("seed default object_model connection");

    let manager = Arc::new(ObjectStoreManager::new(fixture.object_url.clone()));
    let service = ReportService::new(fixture.server_pool.clone(), manager, connections);

    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path(fixtures_dir().join("__snapshots__"));
    settings.set_prepend_module_to_snapshot(false);

    let mut failed = false;
    for (name, definition) in collect_fixtures() {
        let response = service.validate_report(TENANT_ID, &definition).await;
        let result = std::panic::AssertUnwindSafe(|| {
            settings.bind(|| {
                insta::assert_json_snapshot!(format!("runtime_validate_{name}"), response);
            });
        });
        if std::panic::catch_unwind(result).is_err() {
            failed = true;
        }
    }

    // Only drop databases on success — failures keep state around for review.
    if !failed {
        fixture.cleanup().await;
    } else {
        eprintln!(
            "Snapshot mismatch — temp databases retained for debugging. Run `cargo insta review`."
        );
        panic!("one or more runtime validate snapshots diverged");
    }
}

/// Phase 5 E2E: drive `validate_report` through the MCP tool layer end to
/// end. Builds an axum router with the real REST handler on top of the
/// test DB, wraps it in a [`SmoMcpServer`], and calls
/// `tools::reports::validate_report`. The expected MCP response is
/// `{ valid, mode, errors, warnings, ... }` where `errors` matches what
/// the REST handler returns directly and `warnings` contains any
/// `runtara_report_dsl::lint::lint` output.
///
/// Run together with the other tests in this file because both need the
/// same throwaway tenant DB and migrations.
#[tokio::test]
async fn mcp_validate_report_proxies_rest_and_emits_lint() {
    use axum::Router;
    use axum::routing::post;
    use runtara_server::auth::{AuthContext, AuthMethod};
    use runtara_server::mcp::tools::reports::{
        ReportValidationMode, ValidateReportParams, validate_report as mcp_validate_report,
    };

    let Some(fixture) = DbFixture::start().await else {
        eprintln!("Skipping MCP validate E2E: no test database configured");
        return;
    };

    ensure_config(&fixture.object_url);

    let connections = Arc::new(ConnectionsFacade::new(ConnectionsState::from_config(
        ConnectionsConfig {
            db_pool: fixture.server_pool.clone(),
            redis_manager: None,
            public_base_url: "http://localhost".to_string(),
            http_client: reqwest::Client::new(),
            cipher: Arc::new(NoOpCipher),
            compatibility: Arc::new(IntegrationCompatibility::default()),
            agent_catalog: Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(
                Vec::new(),
            )),
        },
    )));

    let manager = Arc::new(ObjectStoreManager::new(fixture.object_url.clone()));

    // Combined state that the validate handler's three `State<...>`
    // extractors resolve from via the `FromRef` impls below.
    #[derive(Clone)]
    struct TestState {
        pool: PgPool,
        manager: Arc<ObjectStoreManager>,
        connections: Arc<ConnectionsFacade>,
    }
    impl axum::extract::FromRef<TestState> for PgPool {
        fn from_ref(state: &TestState) -> PgPool {
            state.pool.clone()
        }
    }
    impl axum::extract::FromRef<TestState> for Arc<ObjectStoreManager> {
        fn from_ref(state: &TestState) -> Arc<ObjectStoreManager> {
            state.manager.clone()
        }
    }
    impl axum::extract::FromRef<TestState> for Arc<ConnectionsFacade> {
        fn from_ref(state: &TestState) -> Arc<ConnectionsFacade> {
            state.connections.clone()
        }
    }

    let test_state = TestState {
        pool: fixture.server_pool.clone(),
        manager: manager.clone(),
        connections: connections.clone(),
    };

    // Build a minimal internal router that mounts just the validate route
    // on the test state. Real server merges this with many other routers;
    // we only need the one path the MCP tool talks to.
    let internal_router = Router::new()
        .route(
            "/api/runtime/reports/validate",
            post(runtara_server::api::handlers::reports::validate_report),
        )
        .with_state(test_state)
        // Inject an AuthContext on every request so the `OrgId` extractor in
        // the handler resolves the tenant — the production internal router
        // gets this from MCP's `build_request` helper, which we bypass here
        // since `tools::reports::validate_report` calls `api_post` itself.
        .layer(axum::middleware::from_fn(
            |mut request: axum::extract::Request, next: axum::middleware::Next| async move {
                request.extensions_mut().insert(AuthContext::new(
                    TENANT_ID.to_string(),
                    "test-mcp-validate".to_string(),
                    AuthMethod::Jwt,
                ));
                next.run(request).await
            },
        ));

    let server = runtara_server::mcp::server::SmoMcpServer::new(
        fixture.server_pool.clone(),
        manager.clone(),
        None,
        TENANT_ID.to_string(),
        internal_router,
    );

    // Markdown-only fixture has no schema dependency — passes validation
    // cleanly so we can assert `errors: []` from the REST side and `valid: true`.
    let markdown = serde_json::json!({
        "definitionVersion": 1,
        "blocks": [{
            "id": "intro",
            "type": "markdown",
            "markdown": { "content": "# Hello" }
        }]
    });

    // mode=all → REST validation + lint pass merged. We expect `valid: true`
    // and `errors: []` because the fixture is canonical-shape.
    let mut params_all = ValidateReportParams {
        definition: markdown.clone(),
        mode: Some(ReportValidationMode::All),
    };
    let response_all = mcp_validate_report(&server, params_all)
        .await
        .expect("MCP validate_report (mode=all) succeeded");
    let body_all: serde_json::Value =
        serde_json::from_str(extract_text(&response_all)).expect("MCP response JSON");
    assert_eq!(body_all.get("valid"), Some(&serde_json::json!(true)));
    assert_eq!(
        body_all.get("errors"),
        Some(&serde_json::json!([])),
        "MCP errors must match REST errors (both empty for a canonical fixture); got: {body_all}"
    );

    // Definition with an unknown root key triggers the new lint warning.
    let mut definition_lint = markdown.clone();
    definition_lint["filterz"] = serde_json::json!([]);
    params_all = ValidateReportParams {
        definition: definition_lint.clone(),
        mode: Some(ReportValidationMode::All),
    };
    let response_lint = mcp_validate_report(&server, params_all)
        .await
        .expect("MCP validate_report (mode=all) with lint succeeded");
    let body_lint: serde_json::Value =
        serde_json::from_str(extract_text(&response_lint)).expect("MCP response JSON");
    let warnings = body_lint
        .get("warnings")
        .and_then(serde_json::Value::as_array)
        .expect("warnings array");
    assert!(
        warnings
            .iter()
            .any(|w| w.get("code") == Some(&serde_json::json!("UNKNOWN_REPORT_FIELD"))),
        "lint must surface UNKNOWN_REPORT_FIELD for the bogus 'filterz' key; got: {body_lint}"
    );

    // mode=syntax → static JSON-Schema-only path; should not hit REST.
    let params_syntax = ValidateReportParams {
        definition: markdown.clone(),
        mode: Some(ReportValidationMode::Syntax),
    };
    let response_syntax = mcp_validate_report(&server, params_syntax)
        .await
        .expect("MCP validate_report (mode=syntax) succeeded");
    let body_syntax: serde_json::Value =
        serde_json::from_str(extract_text(&response_syntax)).expect("MCP response JSON");
    assert_eq!(body_syntax.get("mode"), Some(&serde_json::json!("syntax")));
    assert_eq!(body_syntax.get("valid"), Some(&serde_json::json!(true)));

    fixture.cleanup().await;
}

/// Pull the JSON text out of an MCP `CallToolResult`. The first content
/// item is the JSON-serialized response.
fn extract_text(result: &rmcp::model::CallToolResult) -> &str {
    let first = result.content.first().expect("at least one content item");
    let raw = first.as_text().expect("content is text");
    &raw.text
}

/// Exercises the new MCP workflow-organization tools end to end through the
/// real REST routes + DB (the same in-process path production uses): the tools
/// call `api_*` against `internal_router`, which dispatches to the actual
/// handlers. Proves `move_workflow`, `list_workflow_folders`, and
/// `delete_workflow` round-trip correctly. Ownership scoping is not re-asserted
/// here — it lives in the handler and is covered by `middleware::authorization`
/// unit tests; this env runs without `MembershipPolicy::Required` (role: None).
#[tokio::test]
async fn mcp_workflow_move_list_folders_delete_round_trip() {
    use axum::Router;
    use axum::extract::FromRef;
    use axum::routing::{get, post, put};
    use runtara_dsl::agent_meta::AgentCatalog;
    use runtara_server::auth::{AuthContext, AuthMethod};
    use runtara_server::mcp::tools::workflows::{
        CreateWorkflowParams, DeleteWorkflowParams, GetWorkflowParams, ListWorkflowFoldersParams,
        MoveWorkflowParams, create_workflow, delete_workflow, get_workflow, list_workflow_folders,
        move_workflow,
    };

    let Some(fixture) = DbFixture::start().await else {
        eprintln!("Skipping MCP workflow folders E2E: no test database configured");
        return;
    };

    ensure_config(&fixture.object_url);

    let agent_catalog = Arc::new(AgentCatalog::from_agents(Vec::new()));
    let connections = Arc::new(ConnectionsFacade::new(ConnectionsState::from_config(
        ConnectionsConfig {
            db_pool: fixture.server_pool.clone(),
            redis_manager: None,
            public_base_url: "http://localhost".to_string(),
            http_client: reqwest::Client::new(),
            cipher: Arc::new(NoOpCipher),
            compatibility: Arc::new(IntegrationCompatibility::default()),
            agent_catalog: agent_catalog.clone(),
        },
    )));
    let manager = Arc::new(ObjectStoreManager::new(fixture.object_url.clone()));

    // State the workflow handlers' `State<...>` extractors resolve from.
    #[derive(Clone)]
    struct WfState {
        pool: PgPool,
        connections: Arc<ConnectionsFacade>,
        agent_catalog: Arc<AgentCatalog>,
        events: runtara_server::product_events::ProductEventSink,
    }
    impl FromRef<WfState> for PgPool {
        fn from_ref(s: &WfState) -> PgPool {
            s.pool.clone()
        }
    }
    impl FromRef<WfState> for Arc<ConnectionsFacade> {
        fn from_ref(s: &WfState) -> Arc<ConnectionsFacade> {
            s.connections.clone()
        }
    }
    impl FromRef<WfState> for Arc<AgentCatalog> {
        fn from_ref(s: &WfState) -> Arc<AgentCatalog> {
            s.agent_catalog.clone()
        }
    }
    impl FromRef<WfState> for runtara_server::product_events::ProductEventSink {
        fn from_ref(s: &WfState) -> runtara_server::product_events::ProductEventSink {
            s.events.clone()
        }
    }

    // Product-event sink for the mounted handlers. The receiver is kept alive (channel open)
    // so emits buffer harmlessly; the test asserts on workflow behavior, not on events.
    let (events_tx, _events_rx) = tokio::sync::mpsc::channel(64);
    let wf_state = WfState {
        pool: fixture.server_pool.clone(),
        connections: connections.clone(),
        agent_catalog: agent_catalog.clone(),
        events: runtara_server::product_events::ProductEventSink::new(events_tx),
    };

    // Mount only the workflow routes the tools talk to. A middleware layer
    // injects the caller's AuthContext on every request (production gets this
    // from MCP's `build_request`, bypassed here because the tools call `api_*`).
    let internal_router = Router::new()
        .route(
            "/api/runtime/workflows/create",
            post(runtara_server::api::handlers::workflows::create_workflow_handler),
        )
        .route(
            "/api/runtime/workflows/folders",
            get(runtara_server::api::handlers::workflows::list_folders_handler),
        )
        .route(
            "/api/runtime/workflows/{id}",
            get(runtara_server::api::handlers::workflows::get_workflow_handler),
        )
        .route(
            "/api/runtime/workflows/{id}/move",
            put(runtara_server::api::handlers::workflows::move_workflow_handler),
        )
        .route(
            "/api/runtime/workflows/{id}/delete",
            post(runtara_server::api::handlers::workflows::delete_workflow_handler),
        )
        .with_state(wf_state)
        .layer(axum::middleware::from_fn(
            |mut request: axum::extract::Request, next: axum::middleware::Next| async move {
                request.extensions_mut().insert(AuthContext::new(
                    TENANT_ID.to_string(),
                    "owner-user".to_string(),
                    AuthMethod::Jwt,
                ));
                next.run(request).await
            },
        ));

    let server = runtara_server::mcp::server::SmoMcpServer::new(
        fixture.server_pool.clone(),
        manager.clone(),
        None,
        TENANT_ID.to_string(),
        internal_router,
    );

    // 1. Create a workflow at root.
    let created = create_workflow(
        &server,
        CreateWorkflowParams {
            name: "MCP Folder Test".to_string(),
            description: "round-trip".to_string(),
        },
    )
    .await
    .expect("create_workflow");
    let created_body: serde_json::Value =
        serde_json::from_str(extract_text(&created)).expect("create JSON");
    let wf_id = created_body["data"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("missing workflow id in create response: {created_body:#}"))
        .to_string();

    // 2. Move it into /Sales/.
    let moved = move_workflow(
        &server,
        MoveWorkflowParams {
            workflow_id: wf_id.clone(),
            path: "/Sales/".to_string(),
        },
    )
    .await
    .expect("move_workflow");
    // move_workflow_handler wraps MoveWorkflowResponse in the ApiResponse
    // envelope: {success, message, data: {success, workflowId, path}}.
    let moved_body: serde_json::Value =
        serde_json::from_str(extract_text(&moved)).expect("move JSON");
    assert_eq!(
        moved_body["success"],
        json!(true),
        "move body: {moved_body:#}"
    );
    assert_eq!(
        moved_body["data"]["path"],
        json!("/Sales/"),
        "move body: {moved_body:#}"
    );
    assert_eq!(moved_body["data"]["workflowId"], json!(wf_id));

    // 3. The folder now shows up in list_workflow_folders.
    let folders = list_workflow_folders(&server, ListWorkflowFoldersParams {})
        .await
        .expect("list_workflow_folders");
    let folders_body: serde_json::Value =
        serde_json::from_str(extract_text(&folders)).expect("folders JSON");
    let folder_list = folders_body["folders"]
        .as_array()
        .unwrap_or_else(|| panic!("folders not an array: {folders_body:#}"));
    assert!(
        folder_list.iter().any(|p| p == &json!("/Sales/")),
        "expected '/Sales/' in folders: {folders_body:#}"
    );

    // 4. Soft-delete the workflow.
    let deleted = delete_workflow(
        &server,
        DeleteWorkflowParams {
            workflow_id: wf_id.clone(),
        },
    )
    .await
    .expect("delete_workflow");
    let deleted_body: serde_json::Value =
        serde_json::from_str(extract_text(&deleted)).expect("delete JSON");
    assert_eq!(
        deleted_body["success"],
        json!(true),
        "delete body: {deleted_body:#}"
    );
    assert_eq!(deleted_body["workflowId"], json!(wf_id));

    // 5. After deletion the workflow is no longer fetchable (404 → MCP error).
    let after = get_workflow(
        &server,
        GetWorkflowParams {
            workflow_id: wf_id.clone(),
            version: None,
            compact: None,
        },
    )
    .await;
    assert!(
        after.is_err(),
        "deleted workflow must not be fetchable, got: {after:?}"
    );

    fixture.cleanup().await;
}

// The Phase 6 `per_op_handlers_match_edit_batched_equivalent` test
// was deleted in Phase 8 alongside the legacy per-op REST + service
// handlers. The dsl-level
// `batched_ops_equivalent_to_sequential_application` test in
// `runtara-report-dsl::edit_ops::tests` covers the same equivalence at
// the pure-data layer that's now the only mutation pipeline.
