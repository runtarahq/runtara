//! Sealed credentials with seal as a process of its own: seal's service, on a
//! Unix socket, holds the keys, and this side only asks it. A tenant's keyset
//! is made, used and destroyed through seal's calls.

#![cfg(unix)]

use std::path::PathBuf;

use runtara_connections::sealed::mode::SealMode;
use runtara_connections::sealed::{Allowances, SealedCredentials};

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
