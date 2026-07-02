//! Reports render corpus tests — Phase 0 of the reports refactor.
//!
//! Persists each fixture into the database (bypassing the validator via the
//! repository so fixtures whose schemas/workflows don't exist still get
//! stored), then calls `ReportService::render_report` with a fixed filter
//! state and snapshots the response. Timestamps and UUIDs are masked so the
//! snapshots are deterministic.
//!
//! This is the second drift-detection layer for the refactor: validation
//! lives in `reports_runtime_corpus.rs`; rendering — including the
//! per-block-error path that most fixtures currently hit — lives here.
//!
//! Requires Postgres (same env-var pattern as `reports_runtime_corpus.rs`).

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, OnceLock};

use chrono::Utc;
use runtara_connections::crypto::noop::NoOpCipher;
use runtara_connections::{
    ConnectionsConfig, ConnectionsFacade, ConnectionsState, IntegrationCompatibility,
};
use runtara_server::api::dto::reports::{
    ReportDefinition, ReportDto, ReportRenderRequest, ReportStatus,
};
use runtara_server::api::repositories::object_model::ObjectStoreManager;
use runtara_server::api::repositories::reports::ReportRepository;
use runtara_server::api::services::reports::ReportService;
use runtara_server::config::Config;
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::PgConnectOptions;
use uuid::Uuid;

const TENANT_ID: &str = "tenant_reports_render";

static CONFIG_INIT: OnceLock<()> = OnceLock::new();

fn ensure_config(object_url: &str) {
    CONFIG_INIT.get_or_init(|| {
        unsafe {
            std::env::set_var("TENANT_ID", TENANT_ID);
            std::env::set_var("OBJECT_MODEL_DATABASE_URL", object_url);
        }
        let config = Config::from_env().expect("build test Config");
        runtara_server::config::init(config);
    });
}

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
        let opts = PgConnectOptions::from_str(&base).ok()?;
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
        let server_db = format!("runtara_reports_render_{suffix}");
        let object_db = format!("runtara_reports_render_object_{suffix}");

        let admin_url = format!("postgres://{auth}@{host}:{port}/{admin_db}");
        let admin_pool = match PgPool::connect(&admin_url).await {
            Ok(p) => p,
            Err(e) => {
                eprintln!("connect to admin Postgres: {e}");
                return None;
            }
        };

        sqlx::query(&format!("CREATE DATABASE \"{server_db}\""))
            .execute(&admin_pool)
            .await
            .ok()?;
        sqlx::query(&format!("CREATE DATABASE \"{object_db}\""))
            .execute(&admin_pool)
            .await
            .ok()?;
        admin_pool.close().await;

        let server_url = format!("postgres://{auth}@{host}:{port}/{server_db}");
        let object_url = format!("postgres://{auth}@{host}:{port}/{object_db}");
        let server_pool = PgPool::connect(&server_url).await.ok()?;
        sqlx::migrate!("./migrations")
            .run(&server_pool)
            .await
            .expect("apply server migrations");

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

/// Recursively sort object keys so JSON snapshots are stable across runs.
/// The render response uses `HashMap` for `resolvedFilters` and `blocks`,
/// both of which iterate in non-deterministic order.
fn canonicalize(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<String, Value> = map.into_iter().collect();
            Value::Object(
                sorted
                    .into_iter()
                    .map(|(k, v)| (k, canonicalize(v)))
                    .collect(),
            )
        }
        Value::Array(arr) => Value::Array(arr.into_iter().map(canonicalize).collect()),
        other => other,
    }
}

/// Default filter values applied to every fixture. Chosen so the resolved
/// filter snapshot is deterministic — the `last_30_days` preset would
/// resolve to "now minus 30 days" otherwise.
fn fixed_filters() -> HashMap<String, Value> {
    let mut m = HashMap::new();
    m.insert(
        "date_range".to_string(),
        json!({
            "from": "2026-01-01T00:00:00Z",
            "to":   "2026-02-01T00:00:00Z",
        }),
    );
    m.insert("interval".to_string(), json!("1h"));
    m.insert("status".to_string(), json!(["active"]));
    m.insert("customer_id".to_string(), json!("cust_001"));
    m.insert("selected_order_id".to_string(), json!("ord_001"));
    m.insert("customer_tier".to_string(), json!("gold"));
    m.insert("workflow".to_string(), json!("wf_test"));
    m
}

#[tokio::test]
async fn render_report_snapshots() {
    let Some(fixture) = DbFixture::start().await else {
        eprintln!(
            "Skipping reports render corpus: set TEST_REPORTS_DATABASE_URL or RUNTARA_DATABASE_URL"
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
            connection_events: None,
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
    let repo = ReportRepository::new(fixture.server_pool.clone());

    // Persist every fixture directly via the repository so we bypass the
    // validator — most fixtures reference object-model schemas we haven't
    // seeded, which would otherwise fail `create_report`.
    let mut report_ids = Vec::new();
    for (name, definition) in collect_fixtures() {
        let now = Utc::now();
        let report = ReportDto {
            id: format!("rep_{name}"),
            slug: name.replace('_', "-"),
            name: name.clone(),
            description: None,
            tags: vec![],
            status: ReportStatus::Published,
            definition_version: definition.definition_version,
            definition,
            needs_re_authoring: None,
            created_at: now,
            updated_at: now,
        };
        repo.create(TENANT_ID, &report, None)
            .await
            .unwrap_or_else(|e| panic!("{name}: persist fixture: {e}"));
        report_ids.push((name, report.id));
    }

    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path(fixtures_dir().join("__snapshots__"));
    settings.set_prepend_module_to_snapshot(false);
    // Mask non-deterministic IDs and timestamps so snapshots are stable
    // across runs. The `_id`/`_url` filters cover error messages that quote
    // the seeded connection's UUID.
    settings.add_filter(
        r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}",
        "[uuid]",
    );
    settings.add_filter(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z", "[ts]");

    let mut failed = false;
    for (name, report_id) in report_ids {
        let response = service
            .render_report(
                TENANT_ID,
                &report_id,
                ReportRenderRequest {
                    filters: fixed_filters(),
                    blocks: None,
                    timezone: Some("UTC".to_string()),
                },
            )
            .await;

        let snapshot_body = match response {
            Ok(value) => canonicalize(serde_json::to_value(&value).expect("serialize render")),
            Err(error) => json!({ "error": error.to_string() }),
        };
        let take_snapshot = std::panic::AssertUnwindSafe(|| {
            settings.bind(|| {
                insta::assert_json_snapshot!(format!("render_{name}"), snapshot_body);
            });
        });
        if std::panic::catch_unwind(take_snapshot).is_err() {
            failed = true;
        }
    }

    if !failed {
        fixture.cleanup().await;
    } else {
        eprintln!(
            "Snapshot mismatch — temp databases retained for debugging. Run `cargo insta review`."
        );
        panic!("one or more render snapshots diverged");
    }
}
