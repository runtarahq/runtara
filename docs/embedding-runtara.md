# Embedding Runtara in Your Product

This guide covers how to embed `runtara-environment` into your product with a dedicated database.

## Database Setup

Runtara requires its own PostgreSQL database, separate from your product's database.

```sql
-- Connect as superuser (postgres)
-- psql -U postgres

-- Create dedicated user
CREATE USER runtara WITH PASSWORD 'your_secure_password_here';

-- Create database owned by runtara user
CREATE DATABASE runtara OWNER runtara;

-- Connect to the new database
\c runtara

-- Grant all privileges (owner already has them, but explicit for clarity)
GRANT ALL PRIVILEGES ON DATABASE runtara TO runtara;

-- Allow user to create schemas (for migrations)
GRANT CREATE ON DATABASE runtara TO runtara;

-- Grant usage on public schema
GRANT ALL ON SCHEMA public TO runtara;

-- Set default privileges for future tables
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON TABLES TO runtara;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON SEQUENCES TO runtara;
```

Or as a one-liner for automation:

```bash
psql -U postgres -c "CREATE USER runtara WITH PASSWORD 'your_secure_password_here';" \
  && psql -U postgres -c "CREATE DATABASE runtara OWNER runtara;"
```

Connection string format:
```
postgres://runtara:your_secure_password_here@localhost:5432/runtara
```

## Dependencies

Add to your `Cargo.toml`:

```toml
[dependencies]
runtara-environment = "1.3"
sqlx = { version = "0.8", features = ["runtime-tokio", "postgres"] }
tokio = { version = "1", features = ["full"] }
```

## Running Migrations

Call `runtara_environment::migrations::run()` once at startup. This automatically runs both core and environment migrations as a single unified set.

```rust
use sqlx::postgres::PgPoolOptions;

async fn setup_runtara() -> anyhow::Result<sqlx::PgPool> {
    // Connect to Runtara's dedicated database
    let runtara_pool = PgPoolOptions::new()
        .max_connections(10)
        .connect("postgres://runtara_user:secure_password@localhost/runtara_db")
        .await?;

    // Run migrations (core + environment combined)
    runtara_environment::migrations::run(&runtara_pool).await?;

    Ok(runtara_pool)
}
```

## Embedding the Runtime

Use `EnvironmentRuntime` to run runtara-environment as part of your application:

```rust
use std::sync::Arc;
use runtara_environment::runtime::EnvironmentRuntime;
use runtara_environment::runner::oci::OciRunner;
use runtara_core::persistence::PostgresPersistence;

async fn start_runtara(pool: sqlx::PgPool) -> anyhow::Result<EnvironmentRuntime> {
    // Create runner (OCI containers)
    let runner = Arc::new(OciRunner::from_env());

    // Create persistence layer (shared with core)
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));

    // Build and start runtime
    let runtime = EnvironmentRuntime::builder()
        .pool(pool)
        .runner(runner)
        .core_persistence(persistence)
        .core_addr("127.0.0.1:8001")      // Where instances connect
        .bind_addr("0.0.0.0:8002".parse()?) // Management API
        .data_dir("/var/lib/runtara")      // Images, bundles, I/O
        .build()?
        .start()
        .await?;

    Ok(runtime)
}
```

## Complete Example

```rust
use std::sync::Arc;
use sqlx::postgres::PgPoolOptions;
use runtara_environment::runtime::EnvironmentRuntime;
use runtara_environment::runner::oci::OciRunner;
use runtara_core::persistence::PostgresPersistence;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Connect to Runtara's dedicated database
    let runtara_pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&std::env::var("RUNTARA_DATABASE_URL")?)
        .await?;

    // 2. Run migrations
    runtara_environment::migrations::run(&runtara_pool).await?;

    // 3. Create components
    let runner = Arc::new(OciRunner::from_env());
    let persistence = Arc::new(PostgresPersistence::new(runtara_pool.clone()));

    // 4. Start runtime
    let runtime = EnvironmentRuntime::builder()
        .pool(runtara_pool)
        .runner(runner)
        .core_persistence(persistence)
        .core_addr("127.0.0.1:8001")
        .bind_addr("0.0.0.0:8002".parse()?)
        .data_dir("/var/lib/runtara")
        .build()?
        .start()
        .await?;

    println!("Runtara environment ready on port 8002");

    // 5. Wait for shutdown signal
    tokio::signal::ctrl_c().await?;

    // 6. Graceful shutdown
    runtime.shutdown().await?;

    Ok(())
}
```

## Configuration Options

| Option | Description |
|--------|-------------|
| `pool` | PostgreSQL connection pool (required) |
| `runner` | Container runner - `OciRunner` or `MockRunner` (required) |
| `core_persistence` | Shared persistence for checkpoints/signals (required) |
| `core_addr` | Address instances use to connect to core (default: `127.0.0.1:8001`) |
| `bind_addr` | QUIC server bind address (default: `0.0.0.0:8002`) |
| `data_dir` | Directory for images, bundles, instance I/O (default: `.data`) |
| `wake_poll_interval` | How often to check for sleeping instances (default: 5s) |
| `wake_batch_size` | Max instances to wake per poll (default: 10) |

## Environment Variables for OCI Runner

When using `OciRunner::from_env()`:

| Variable | Default | Description |
|----------|---------|-------------|
| `BUNDLES_DIR` | `${data_dir}/bundles` | OCI bundle storage |
| `EXECUTION_TIMEOUT_SECS` | `300` | Container timeout |
| `USE_SYSTEMD_CGROUP` | `false` | Use systemd for cgroups |
| `RUNTARA_NETWORK_MODE` | `host` | Network mode: `host`, `pasta`, `none` |

## Using the Management SDK

Once embedded, use `runtara-management-sdk` to interact with Runtara:

```rust
use runtara_management_sdk::{ManagementSdk, SdkConfig, RegisterImageOptions, StartInstanceOptions};

// Connect to embedded Runtara
let sdk = ManagementSdk::new(SdkConfig::new("127.0.0.1:8002"))?;
sdk.connect().await?;

// Register an image
let binary = std::fs::read("./my-workflow")?;
let image = sdk.register_image(
    RegisterImageOptions::new("tenant-1", "my-workflow", binary)
).await?;

// Start an instance
let instance = sdk.start_instance(
    StartInstanceOptions::new(&image.image_id, "tenant-1")
        .with_input(serde_json::json!({"order_id": "123"}))
).await?;

// Query status
let status = sdk.get_instance_status(&instance.instance_id).await?;
```

## Migration Architecture

The migration system uses inheritance:

```
runtara-core (001_*, 002_*, ...)
    └── runtara-environment (20250101000000_*)
```

When you call `runtara_environment::migrations::run()`:
1. Core migrations are collected from `runtara_core::migrations::POSTGRES`
2. Environment migrations are collected from the embedded migrator
3. Both sets are merged and sorted by version
4. SQLx runs them as a single unified migration set

This means:
- One `_sqlx_migrations` table tracks everything
- No conflicts between core and environment
- Safe to run multiple times (idempotent)
- Products don't need to manage migrations separately
