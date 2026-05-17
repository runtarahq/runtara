//! Reports runtime corpus tests — Phase 0 of the reports refactor.
//!
//! Runs every fixture through `ReportService::validate_report`, including
//! semantic checks (schema/workflow lookups). Snapshots the response so any
//! drift surfaces in `cargo insta review` during later phases.
//!
//! Requires a running Postgres with `pgvector` and `pg_trgm` extensions.
//! Reads `TEST_REPORTS_DATABASE_URL` or falls back to `RUNTARA_DATABASE_URL`
//! / `DATABASE_URL`. Skips gracefully when none is set or unreachable.
//!
//! The test creates a UUID-suffixed throwaway database for each run, applies
//! server migrations, runs the corpus, and drops the database on success.
//! On failure the database is left intact for debugging.

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use runtara_connections::crypto::noop::NoOpCipher;
use runtara_connections::{ConnectionsConfig, ConnectionsFacade, ConnectionsState};
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
        "DATABASE_URL",
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
                request.extensions_mut().insert(AuthContext {
                    org_id: TENANT_ID.to_string(),
                    user_id: "test-mcp-validate".to_string(),
                    auth_method: AuthMethod::Jwt,
                });
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

/// Phase 6 acceptance: the five legacy per-block REST/service handlers
/// (add/replace/patch/move/remove) must produce the same persisted
/// state as the equivalent single-op `/edit` batch — they're now
/// `apply_edit_ops`-backed shims. Sequence two flows over the same
/// base definition and assert the resulting definitions are identical.
#[tokio::test]
async fn per_op_handlers_match_edit_batched_equivalent() {
    use runtara_report_dsl::edit_ops::{BlockPosition, ReportEditOp};
    use runtara_server::api::dto::reports::{
        AddReportBlockRequest, CreateReportRequest, MoveReportBlockRequest,
        PatchReportBlockRequest, RemoveReportBlockRequest, ReplaceReportBlockRequest,
        ReportBlockDefinition, ReportBlockPosition,
    };

    let Some(fixture) = DbFixture::start().await else {
        eprintln!("Skipping per-op vs edit equivalence: no test database configured");
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
        },
    )));
    let manager = Arc::new(ObjectStoreManager::new(fixture.object_url.clone()));
    let service = ReportService::new(
        fixture.server_pool.clone(),
        manager.clone(),
        connections.clone(),
    );

    // Base definition: a single markdown block. Schema lookups not
    // required so we can compare apples to apples without seeding object
    // model schemas.
    let base_definition: ReportDefinition = serde_json::from_value(json!({
        "definitionVersion": 1,
        "blocks": [
            { "id": "first", "type": "markdown", "markdown": { "content": "hello" } }
        ]
    }))
    .unwrap();

    let new_block_a = serde_json::from_value(json!({
        "id": "second",
        "type": "markdown",
        "markdown": { "content": "added" }
    }))
    .unwrap();
    let new_block_b = serde_json::from_value(json!({
        "id": "third",
        "type": "markdown",
        "markdown": { "content": "third" }
    }))
    .unwrap();
    let replacement = serde_json::from_value::<ReportBlockDefinition>(json!({
        "id": "first",
        "type": "markdown",
        "markdown": { "content": "replaced" }
    }))
    .unwrap();

    // -------- Per-op path --------
    let per_op_report = service
        .create_report(
            TENANT_ID,
            CreateReportRequest {
                name: "Per-op".to_string(),
                slug: Some("per-op".to_string()),
                description: None,
                tags: vec![],
                status: runtara_server::api::dto::reports::ReportStatus::Draft,
                definition: base_definition.clone(),
            },
        )
        .await
        .expect("create per-op report");

    service
        .add_report_block(
            TENANT_ID,
            &per_op_report.id,
            AddReportBlockRequest {
                block: serde_json::from_value(new_block_a).unwrap(),
                position: Some(ReportBlockPosition {
                    after_block_id: Some("first".to_string()),
                    ..Default::default()
                }),
            },
        )
        .await
        .expect("add second");
    service
        .add_report_block(
            TENANT_ID,
            &per_op_report.id,
            AddReportBlockRequest {
                block: serde_json::from_value(new_block_b).unwrap(),
                position: None,
            },
        )
        .await
        .expect("add third");
    service
        .replace_report_block(
            TENANT_ID,
            &per_op_report.id,
            "first",
            ReplaceReportBlockRequest {
                block: replacement.clone(),
            },
        )
        .await
        .expect("replace first");
    service
        .patch_report_block(
            TENANT_ID,
            &per_op_report.id,
            "first",
            PatchReportBlockRequest {
                patch: json!({ "title": "Heading" }),
            },
        )
        .await
        .expect("patch first");
    service
        .move_report_block(
            TENANT_ID,
            &per_op_report.id,
            "third",
            MoveReportBlockRequest {
                position: ReportBlockPosition {
                    index: Some(0),
                    ..Default::default()
                },
            },
        )
        .await
        .expect("move third");
    service
        .remove_report_block(
            TENANT_ID,
            &per_op_report.id,
            "second",
            RemoveReportBlockRequest {},
        )
        .await
        .expect("remove second");
    let per_op_final = service
        .get_report(TENANT_ID, &per_op_report.id)
        .await
        .expect("reload per-op");

    // -------- Batched edit path --------
    let batched_report = service
        .create_report(
            TENANT_ID,
            CreateReportRequest {
                name: "Batched".to_string(),
                slug: Some("batched".to_string()),
                description: None,
                tags: vec![],
                status: runtara_server::api::dto::reports::ReportStatus::Draft,
                definition: base_definition.clone(),
            },
        )
        .await
        .expect("create batched report");

    let new_block_a = serde_json::from_value(json!({
        "id": "second",
        "type": "markdown",
        "markdown": { "content": "added" }
    }))
    .unwrap();
    let new_block_b = serde_json::from_value(json!({
        "id": "third",
        "type": "markdown",
        "markdown": { "content": "third" }
    }))
    .unwrap();

    service
        .edit_report(
            TENANT_ID,
            &batched_report.id,
            &[
                ReportEditOp::AddBlock {
                    block: serde_json::from_value(new_block_a).unwrap(),
                    position: BlockPosition {
                        after_id: Some("first".to_string()),
                        ..Default::default()
                    },
                },
                ReportEditOp::AddBlock {
                    block: serde_json::from_value(new_block_b).unwrap(),
                    position: BlockPosition::default(),
                },
                ReportEditOp::ReplaceBlock {
                    block_id: "first".to_string(),
                    block: replacement,
                },
                ReportEditOp::PatchBlock {
                    block_id: "first".to_string(),
                    patch: json!({ "title": "Heading" }),
                },
                ReportEditOp::MoveBlock {
                    block_id: "third".to_string(),
                    position: BlockPosition {
                        index: Some(0),
                        ..Default::default()
                    },
                },
                ReportEditOp::RemoveBlock {
                    block_id: "second".to_string(),
                },
            ],
        )
        .await
        .expect("batched edit");
    let batched_final = service
        .get_report(TENANT_ID, &batched_report.id)
        .await
        .expect("reload batched");

    assert_eq!(
        serde_json::to_value(&per_op_final.definition).unwrap(),
        serde_json::to_value(&batched_final.definition).unwrap(),
        "per-op and batched edit must produce identical definitions"
    );

    fixture.cleanup().await;
}
