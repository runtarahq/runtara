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
