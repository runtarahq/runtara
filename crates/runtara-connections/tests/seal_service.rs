//! Sealed credentials with seal as a process of its own: seal's service, on a
//! Unix socket, holds the keys, and this side only asks it. A tenant's keyset
//! is made, used and destroyed through seal's calls, and the keys a server
//! kept in its database are handed over to seal's key directory.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use runtara_connections::sealed::keys::{PgKeySource, export_keys, export_wrapping_key};
use runtara_connections::sealed::mode::SealMode;
use runtara_connections::sealed::{
    Allowances, KeysHandedOver, SealedCredentials, forget_keys_seal_holds, keys_handed_over,
};
use sqlx::PgPool;
use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

const TENANT: &str = "acme";
const SECRET: &str = "s3cr3t-CANARY-token";

/// seal's service, running until it is dropped.
struct Seal {
    socket: PathBuf,
    keys: PathBuf,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    _dir: tempfile::TempDir,
}

impl Seal {
    /// A directory for one seal: short, because a socket's path is.
    fn dir() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("rs")
            .tempdir_in("/tmp")
            .unwrap()
    }

    /// seal with a key directory and wrapping key of its own.
    async fn fresh() -> Self {
        let dir = Self::dir();
        let (keys, kek) = (dir.path().join("keys"), dir.path().join("kek"));
        std::fs::create_dir(&keys).unwrap();
        seal::cli::create_wrapping_key(&kek).unwrap();
        Self::over(dir, keys, kek).await
    }

    /// seal over `keys`, wrapped by the key in `kek`, both inside `dir`.
    async fn over(dir: tempfile::TempDir, keys: PathBuf, kek: PathBuf) -> Self {
        use std::os::unix::fs::MetadataExt;
        let uid = std::fs::metadata(dir.path()).unwrap().uid();
        let socket = dir.path().join("seal.sock");
        let config = format!(
            r#"[listen]
unix = {socket:?}

# The server: every tenant's keyset, which it makes, reads the public keys of,
# and destroys when a tenant leaves.
[[callers]]
name = "runtara-server"
unix_uid = {uid}
keysets = ["*"]
admin = true
destroy_keysets = true

[keys]
source = {{ dir = {keys:?} }}
wrap = {{ file = {kek:?} }}
lock_memory = "off"

[audit]
sink = {{ file = {audit:?} }}
"#,
            socket = socket.display().to_string(),
            keys = keys.display().to_string(),
            kek = kek.display().to_string(),
            audit = dir.path().join("audit.jsonl").display().to_string(),
        );
        let config = seal::config::Config::parse(&config).expect("seal's configuration");
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let (ready, listening) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            let outcome = seal::serve::run(
                config,
                move |_| {
                    let _ = ready.send(());
                },
                async {
                    let _ = stopped.await;
                },
            )
            .await;
            if let Err(error) = outcome {
                panic!("seal did not run: {error}");
            }
        });
        listening.await.expect("seal listens");
        Self {
            socket,
            keys,
            stop: Some(stop),
            _dir: dir,
        }
    }

    /// This side of it, as a server told `RUNTARA_SEAL_MODE=unix` has it.
    fn client(&self) -> SealedCredentials {
        let mode = SealMode::Unix {
            sockets: vec![self.socket.clone()],
        };
        SealedCredentials::remote(
            mode.client().unwrap().expect("a client for seal elsewhere"),
            Allowances::default(),
        )
    }

    fn key_files(&self, keyset: &str) -> usize {
        std::fs::read_dir(self.keys.join(keyset))
            .map(|entries| entries.count())
            .unwrap_or(0)
    }
}

impl Drop for Seal {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tenants_keys_are_made_used_and_destroyed_by_seal_alone() {
    let seal = Seal::fresh().await;
    let sealed = seal.client();

    assert_eq!(sealed.mode(), "unix");
    let info = sealed.reach().await.expect("seal answers");
    assert_eq!(info.mode.as_str(), "unix");

    // Sealing a tenant's first secret makes its keyset, in seal's directory.
    assert_eq!(seal.key_files(TENANT), 0);
    let marker = sealed
        .seal_webhook_secret(TENANT, "c-1", None, SECRET)
        .await
        .unwrap();
    assert_eq!(seal.key_files(TENANT), 1, "the key is seal's to keep");
    assert!(
        !marker.to_string().contains(SECRET),
        "what is stored does not hold the secret"
    );

    // seal checks a token against the sealed one, and answers yes or no.
    assert_eq!(
        sealed
            .verify_webhook_secret(TENANT, "c-1", &marker, SECRET)
            .await,
        Ok(true)
    );
    assert_eq!(
        sealed
            .verify_webhook_secret(TENANT, "c-1", &marker, "another-token")
            .await,
        Ok(false)
    );
    // A second secret is sealed to the same key.
    sealed
        .seal_webhook_secret(TENANT, "c-2", None, SECRET)
        .await
        .unwrap();
    assert_eq!(seal.key_files(TENANT), 1);

    // A tenant that leaves has its keys destroyed, by seal, and nothing
    // sealed for it opens again.
    sealed.destroy_tenant_keys(TENANT).await.unwrap();
    assert_eq!(seal.key_files(TENANT), 0);
    assert!(
        sealed
            .verify_webhook_secret(TENANT, "c-1", &marker, SECRET)
            .await
            .is_err()
    );
    // Destroying them again finds nothing to do.
    sealed.destroy_tenant_keys(TENANT).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_seal_that_does_not_answer_is_found_when_it_is_asked() {
    let dir = Seal::dir();
    let mode = SealMode::Unix {
        sockets: vec![dir.path().join("nobody.sock")],
    };
    let sealed = SealedCredentials::remote(mode.client().unwrap().unwrap(), Allowances::default());
    assert!(sealed.reach().await.is_err());
}

async fn key_table() -> (PgPool, ContainerAsync<Postgres>) {
    let container = Postgres::default()
        .start()
        .await
        .expect("required Docker Postgres container must start");
    let url = format!(
        "postgres://postgres:postgres@{}:{}/postgres",
        container.get_host().await.unwrap(),
        container.get_host_port_ipv4(5432).await.unwrap()
    );
    let pool = PgPool::connect(&url).await.unwrap();
    // As the server's migration makes it.
    sqlx::query(
        "CREATE TABLE seal_wrapped_key (
            keyset TEXT NOT NULL,
            kid TEXT NOT NULL,
            tenant_id TEXT NOT NULL,
            public_key TEXT NOT NULL,
            created BIGINT NOT NULL,
            wrap_id TEXT NOT NULL,
            wrapped TEXT NOT NULL,
            stored_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            PRIMARY KEY (keyset, kid)
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    (pool, container)
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(files_under(&path));
        } else {
            files.push(path);
        }
    }
    files
}

/// A deployment that ran seal in its server moves to a seal of its own: the
/// keys go from its database to seal's directory, with the key that wraps
/// them, and what was sealed before opens there.
#[tokio::test(flavor = "multi_thread")]
async fn keys_a_server_kept_are_handed_over_to_a_seal_of_its_own() {
    let (pool, _postgres) = key_table().await;
    let wrapping_key = [7_u8; 32];
    let in_server = SealedCredentials::in_process(
        Arc::new(PgKeySource::new(pool.clone())),
        &wrapping_key,
        Allowances::default(),
    )
    .unwrap();
    let marker = in_server
        .seal_webhook_secret(TENANT, "c-1", None, SECRET)
        .await
        .unwrap();
    let other = in_server
        .seal_webhook_secret("another-tenant", "c-9", None, SECRET)
        .await
        .unwrap();

    let dir = Seal::dir();
    let (keys, kek) = (dir.path().join("keys"), dir.path().join("kek"));
    assert_eq!(export_keys(&pool, &keys).await.unwrap(), (2, 0));
    // Run again, it finds them there and replaces nothing.
    assert_eq!(export_keys(&pool, &keys).await.unwrap(), (0, 2));
    export_wrapping_key(&kek, &wrapping_key).unwrap();
    assert!(
        export_wrapping_key(&kek, &wrapping_key).is_err(),
        "a wrapping key file is never written over"
    );
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&kek), 0o600);
        assert_eq!(mode(&keys), 0o700);
    }
    // The directory holds the keys wrapped, as the table did: neither the
    // secret nor the wrapping key is in it.
    for file in files_under(&keys) {
        let content = std::fs::read(&file).unwrap();
        assert!(
            !content
                .windows(SECRET.len())
                .any(|w| w == SECRET.as_bytes())
        );
        assert!(!content.windows(32).any(|w| w == wrapping_key));
    }

    // A seal that was not given the keys is told apart from one that was,
    // before a server starts on it and seals to new keys.
    let empty = Seal::fresh().await;
    assert!(matches!(
        keys_handed_over(&pool, &empty.client()).await,
        Ok(KeysHandedOver::Missing { .. })
    ));
    drop(empty);

    let seal = Seal::over(dir, keys, kek).await;
    let sealed = seal.client();
    assert_eq!(
        keys_handed_over(&pool, &sealed).await,
        Ok(KeysHandedOver::All(2))
    );
    assert_eq!(
        sealed
            .verify_webhook_secret(TENANT, "c-1", &marker, SECRET)
            .await,
        Ok(true)
    );
    assert_eq!(
        sealed
            .verify_webhook_secret("another-tenant", "c-9", &other, SECRET)
            .await,
        Ok(true)
    );
    // New secrets are sealed to the key that came over, not a new one.
    sealed
        .seal_webhook_secret(TENANT, "c-2", None, SECRET)
        .await
        .unwrap();
    assert_eq!(seal.key_files(TENANT), 1);

    // The database's copy goes once seal holds every key: not from a server
    // that still runs seal itself, and not while a key has not come over.
    let rows = || async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM seal_wrapped_key")
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    let refused = forget_keys_seal_holds(&pool, &in_server).await.unwrap_err();
    assert!(refused.contains("in_process"), "{refused}");
    // A key made in the database after the export, which seal never got.
    in_server
        .seal_webhook_secret("late-tenant", "c-1", None, SECRET)
        .await
        .unwrap();
    let missing = forget_keys_seal_holds(&pool, &sealed).await.unwrap_err();
    assert!(missing.contains("late-tenant"), "{missing}");
    assert_eq!(rows().await, 3, "nothing is deleted while a key is missing");
    // Exported now, it is there for seal to find. (seal was just asked for
    // that keyset, and remembers for half a minute that it had no key: the
    // rest of this test does not wait for that.)
    assert_eq!(export_keys(&pool, &seal.keys).await.unwrap(), (1, 2));
    sqlx::query("DELETE FROM seal_wrapped_key WHERE keyset = 'late-tenant'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(forget_keys_seal_holds(&pool, &sealed).await, Ok(2));
    assert_eq!(
        keys_handed_over(&pool, &sealed).await,
        Ok(KeysHandedOver::NoneKept)
    );
    assert_eq!(rows().await, 0);
    // What was sealed still opens: seal's copy is the one in use.
    assert_eq!(
        sealed
            .verify_webhook_secret(TENANT, "c-1", &marker, SECRET)
            .await,
        Ok(true)
    );
}
