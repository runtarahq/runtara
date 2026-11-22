// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Where seal runs for this deployment.
//!
//! In this process, the server holds the keys and can open every credential.
//! As a process of its own, on this host over a Unix socket or on another
//! over mutual TLS, seal alone holds them: this process can ask seal to use a
//! credential, toward the destination sealed into it, and cannot read one.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use seal_client::{
    ClientTls, ConnectRequest, Info, Mode, PgStream, PresignRequest, PublicKeyInfo, SealClient,
    SealError, SendRequest, SendResponse, VerifyRequest, WrappedKey, Zeroizing,
};
use seal_format::KeysetId;

/// `in_process` (the default), `unix` or `mtls`.
pub const MODE_ENV: &str = "RUNTARA_SEAL_MODE";
/// With `unix`: seal's socket. Several, separated by commas, are instances of
/// one seal; a request goes to another only if one cannot be reached.
pub const SOCKET_ENV: &str = "RUNTARA_SEAL_SOCKET";
/// With `mtls`: seal's `host:port`. Several, separated by commas, as above.
pub const ADDRESS_ENV: &str = "RUNTARA_SEAL_ADDRESS";
/// With `mtls`: a directory with `ca.pem` (the CA of seal's certificate),
/// and this server's `tls.pem` and `tls.key`, which seal knows it by. The
/// files are read again when they change, so a certificate renewed in place
/// is the one the next connection presents, with no restart.
pub const TLS_DIR_ENV: &str = "RUNTARA_SEAL_TLS_DIR";
/// With `mtls`: the name seal's certificate is for. `seal` if unset.
pub const SERVER_NAME_ENV: &str = "RUNTARA_SEAL_SERVER_NAME";

const DEFAULT_SERVER_NAME: &str = "seal";

/// Where seal runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SealMode {
    /// In this process, with the keys in this deployment's database.
    InProcess,
    /// A process on this host, on these sockets.
    Unix { sockets: Vec<PathBuf> },
    /// A process elsewhere, at these addresses, over mutual TLS.
    Mtls {
        addresses: Vec<String>,
        tls_dir: PathBuf,
        server_name: String,
    },
}

fn listed(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_owned)
        .collect()
}

impl SealMode {
    /// The mode the environment names.
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// The mode `lookup` names, as the environment would.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let set = |name: &str| {
            lookup(name)
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        match set(MODE_ENV).as_deref() {
            None | Some("in_process") => Ok(Self::InProcess),
            Some("unix") => {
                let sockets: Vec<PathBuf> = set(SOCKET_ENV)
                    .map(|value| listed(&value))
                    .unwrap_or_default()
                    .into_iter()
                    .map(PathBuf::from)
                    .collect();
                if sockets.is_empty() {
                    return Err(format!("{MODE_ENV}=unix needs {SOCKET_ENV}, seal's socket"));
                }
                Ok(Self::Unix { sockets })
            }
            Some("mtls") => {
                let addresses = set(ADDRESS_ENV)
                    .map(|value| listed(&value))
                    .unwrap_or_default();
                if addresses.is_empty() {
                    return Err(format!(
                        "{MODE_ENV}=mtls needs {ADDRESS_ENV}, seal's host:port"
                    ));
                }
                if let Some(address) = addresses.iter().find(|address| address.contains("://")) {
                    return Err(format!(
                        "{ADDRESS_ENV} is host:port, with no scheme: `{address}`"
                    ));
                }
                let tls_dir = set(TLS_DIR_ENV).ok_or_else(|| {
                    format!(
                        "{MODE_ENV}=mtls needs {TLS_DIR_ENV}, a directory with ca.pem, tls.pem \
                         and tls.key"
                    )
                })?;
                Ok(Self::Mtls {
                    addresses,
                    tls_dir: PathBuf::from(tls_dir),
                    server_name: set(SERVER_NAME_ENV)
                        .unwrap_or_else(|| DEFAULT_SERVER_NAME.to_owned()),
                })
            }
            Some(other) => Err(format!(
                "{MODE_ENV} is `{other}`; it is `in_process`, `unix` or `mtls`"
            )),
        }
    }

    /// Whether seal is a process of its own.
    pub fn is_remote(&self) -> bool {
        !matches!(self, Self::InProcess)
    }

    /// A client for seal as a process of its own. `None` for seal in this
    /// process, which is built from the keys instead.
    pub fn client(&self) -> Result<Option<Arc<dyn SealClient>>, String> {
        match self {
            Self::InProcess => Ok(None),
            Self::Unix { sockets } => seal_client::unix(sockets)
                .map(|client| Some(Arc::new(client) as Arc<dyn SealClient>))
                .map_err(|error| format!("{SOCKET_ENV}: {error}")),
            Self::Mtls {
                addresses,
                tls_dir,
                server_name,
            } => {
                let (addresses, server_name) = (addresses.clone(), server_name.clone());
                let following = FollowingCertificates::new(tls_dir.clone(), move |files| {
                    let tls = ClientTls {
                        ca_pem: files.ca_pem.clone(),
                        cert_pem: files.cert_pem.clone(),
                        key_pem: files.key_pem.clone(),
                        server_name: server_name.clone(),
                    };
                    seal_client::mtls(&addresses, &tls)
                        .map(|client| Arc::new(client) as Arc<dyn SealClient>)
                        .map_err(|error| format!("{TLS_DIR_ENV}: {error}"))
                })?;
                Ok(Some(Arc::new(following)))
            }
        }
    }
}

/// How often the certificate files are looked at for a change.
const CERTIFICATES_CHECKED_EVERY: Duration = Duration::from_secs(5);

/// The files of [`TLS_DIR_ENV`], as read.
struct CertificateFiles {
    ca_pem: String,
    cert_pem: String,
    key_pem: String,
}

const CERTIFICATE_FILES: [&str; 3] = ["ca.pem", "tls.pem", "tls.key"];

/// What tells one version of the files from the next: when each was last
/// written and how long it is, read through any link. `None` for a file that
/// is not there.
type Stamp = [Option<(SystemTime, u64)>; 3];

fn stamp(tls_dir: &Path) -> Stamp {
    CERTIFICATE_FILES.map(|file| {
        let metadata = std::fs::metadata(tls_dir.join(file)).ok()?;
        Some((metadata.modified().ok()?, metadata.len()))
    })
}

fn read_certificates(tls_dir: &Path) -> Result<CertificateFiles, String> {
    let read = |file: &str| {
        std::fs::read_to_string(tls_dir.join(file))
            .map_err(|error| format!("{TLS_DIR_ENV}: {} cannot be read: {error}", file))
    };
    Ok(CertificateFiles {
        ca_pem: read("ca.pem")?,
        cert_pem: read("tls.pem")?,
        key_pem: read("tls.key")?,
    })
}

type BuildClient = dyn Fn(&CertificateFiles) -> Result<Arc<dyn SealClient>, String> + Send + Sync;

/// seal over mutual TLS, with the certificates as the files have them now.
///
/// A certificate may last hours and be renewed in place. seal checks the one
/// a connection is made with, and ends each connection after a while, so a
/// client that kept the certificate it started with would lose seal when
/// that one expired. This one looks at the files before a call, at most every
/// [`CERTIFICATES_CHECKED_EVERY`], and connects with the new ones once they
/// change. Files that do not load, as when a renewal is half written, leave
/// the client it has in use, and are said once. A call under way keeps the
/// client it started with.
struct FollowingCertificates {
    tls_dir: PathBuf,
    build: Box<BuildClient>,
    current: Mutex<Followed>,
}

struct Followed {
    client: Arc<dyn SealClient>,
    /// The files `client` was built from.
    stamp: Stamp,
    /// Files seen since that did not load, so that they are said once.
    refused: Option<Stamp>,
    checked: Instant,
}

impl FollowingCertificates {
    /// A client from the files as they are. Files that do not load are an
    /// error here: a server that could not reach seal says so at its start.
    fn new(
        tls_dir: PathBuf,
        build: impl Fn(&CertificateFiles) -> Result<Arc<dyn SealClient>, String> + Send + Sync + 'static,
    ) -> Result<Self, String> {
        // The stamp is taken first: files replaced while they are read are
        // then read again at the next look.
        let stamp = stamp(&tls_dir);
        let client = build(&read_certificates(&tls_dir)?)?;
        Ok(Self {
            tls_dir,
            build: Box::new(build),
            current: Mutex::new(Followed {
                client,
                stamp,
                refused: None,
                checked: Instant::now(),
            }),
        })
    }

    /// The client to use now.
    fn client(&self) -> Arc<dyn SealClient> {
        self.client_after(CERTIFICATES_CHECKED_EVERY)
    }

    /// The client to use now, looking at the files if they were last looked
    /// at `every` ago or longer.
    fn client_after(&self, every: Duration) -> Arc<dyn SealClient> {
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if current.checked.elapsed() < every {
            return Arc::clone(&current.client);
        }
        current.checked = Instant::now();
        let now = stamp(&self.tls_dir);
        if now == current.stamp || current.refused == Some(now) {
            return Arc::clone(&current.client);
        }
        match read_certificates(&self.tls_dir).and_then(|files| (self.build)(&files)) {
            Ok(client) => {
                tracing::info!(
                    tls_dir = %self.tls_dir.display(),
                    "seal's client certificates changed; new connections use them"
                );
                current.client = client;
                current.stamp = now;
                current.refused = None;
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    tls_dir = %self.tls_dir.display(),
                    "seal's client certificates changed and do not load; keeping the ones in use"
                );
                current.refused = Some(now);
            }
        }
        Arc::clone(&current.client)
    }
}

#[async_trait]
impl SealClient for FollowingCertificates {
    fn mode(&self) -> Mode {
        Mode::Mtls
    }

    async fn send(&self, request: SendRequest) -> Result<SendResponse, SealError> {
        self.client().send(request).await
    }

    async fn presign(&self, request: PresignRequest) -> Result<Zeroizing<String>, SealError> {
        self.client().presign(request).await
    }

    async fn verify(&self, request: VerifyRequest) -> Result<bool, SealError> {
        self.client().verify(request).await
    }

    async fn connect_pg(&self, request: ConnectRequest) -> Result<PgStream, SealError> {
        self.client().connect_pg(request).await
    }

    async fn info(&self) -> Result<Info, SealError> {
        self.client().info().await
    }

    async fn generate_keyset(&self, keyset: &KeysetId) -> Result<WrappedKey, SealError> {
        self.client().generate_keyset(keyset).await
    }

    async fn rotate(&self, keyset: &KeysetId) -> Result<WrappedKey, SealError> {
        self.client().rotate(keyset).await
    }

    async fn public_keys(&self, keyset: &KeysetId) -> Result<Vec<PublicKeyInfo>, SealError> {
        self.client().public_keys(keyset).await
    }

    async fn destroy_keyset(&self, keyset: &KeysetId) -> Result<(), SealError> {
        self.client().destroy_keyset(keyset).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(pairs: &[(&str, &str)]) -> Result<SealMode, String> {
        SealMode::from_lookup(|name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        })
    }

    #[test]
    fn seal_runs_in_this_process_unless_told_otherwise() {
        assert_eq!(mode(&[]), Ok(SealMode::InProcess));
        assert_eq!(mode(&[(MODE_ENV, " ")]), Ok(SealMode::InProcess));
        assert_eq!(mode(&[(MODE_ENV, "in_process")]), Ok(SealMode::InProcess));
        // What only another mode reads does not choose the mode.
        assert_eq!(
            mode(&[(SOCKET_ENV, "/run/seal/seal.sock")]),
            Ok(SealMode::InProcess)
        );
    }

    #[test]
    fn a_socket_names_seal_on_this_host() {
        assert_eq!(
            mode(&[
                (MODE_ENV, "unix"),
                (SOCKET_ENV, "/run/seal/a.sock, /run/seal/b.sock,")
            ]),
            Ok(SealMode::Unix {
                sockets: vec!["/run/seal/a.sock".into(), "/run/seal/b.sock".into()]
            })
        );
        let missing = mode(&[(MODE_ENV, "unix")]).unwrap_err();
        assert!(missing.contains(SOCKET_ENV), "{missing}");
    }

    #[test]
    fn an_address_and_certificates_name_seal_elsewhere() {
        assert_eq!(
            mode(&[
                (MODE_ENV, "mtls"),
                (ADDRESS_ENV, "seal-1:7443,seal-2:7443"),
                (TLS_DIR_ENV, "/etc/runtara/seal"),
            ]),
            Ok(SealMode::Mtls {
                addresses: vec!["seal-1:7443".to_owned(), "seal-2:7443".to_owned()],
                tls_dir: "/etc/runtara/seal".into(),
                server_name: "seal".to_owned(),
            })
        );
        let named = mode(&[
            (MODE_ENV, "mtls"),
            (ADDRESS_ENV, "10.0.0.5:7443"),
            (TLS_DIR_ENV, "/etc/runtara/seal"),
            (SERVER_NAME_ENV, "seal.internal"),
        ])
        .unwrap();
        assert!(
            matches!(named, SealMode::Mtls { server_name, .. } if server_name == "seal.internal")
        );

        let no_address = mode(&[(MODE_ENV, "mtls"), (TLS_DIR_ENV, "/x")]).unwrap_err();
        assert!(no_address.contains(ADDRESS_ENV), "{no_address}");
        let no_certificates = mode(&[(MODE_ENV, "mtls"), (ADDRESS_ENV, "seal:7443")]).unwrap_err();
        assert!(no_certificates.contains(TLS_DIR_ENV), "{no_certificates}");
        let scheme = mode(&[
            (MODE_ENV, "mtls"),
            (ADDRESS_ENV, "https://seal:7443"),
            (TLS_DIR_ENV, "/x"),
        ])
        .unwrap_err();
        assert!(scheme.contains("no scheme"), "{scheme}");
    }

    #[test]
    fn a_mode_that_is_not_one_is_refused_by_name() {
        let error = mode(&[(MODE_ENV, "library")]).unwrap_err();
        assert!(error.contains("`library`"), "{error}");
    }

    /// A client that stands for the certificate it was built from.
    struct Built(String);

    #[async_trait]
    impl SealClient for Built {
        fn mode(&self) -> Mode {
            Mode::Mtls
        }
        async fn send(&self, _: SendRequest) -> Result<SendResponse, SealError> {
            unreachable!()
        }
        async fn presign(&self, _: PresignRequest) -> Result<Zeroizing<String>, SealError> {
            Ok(Zeroizing::new(self.0.clone()))
        }
        async fn verify(&self, _: VerifyRequest) -> Result<bool, SealError> {
            unreachable!()
        }
        async fn connect_pg(&self, _: ConnectRequest) -> Result<PgStream, SealError> {
            unreachable!()
        }
        async fn info(&self) -> Result<Info, SealError> {
            unreachable!()
        }
        async fn generate_keyset(&self, _: &KeysetId) -> Result<WrappedKey, SealError> {
            unreachable!()
        }
        async fn rotate(&self, _: &KeysetId) -> Result<WrappedKey, SealError> {
            unreachable!()
        }
        async fn public_keys(&self, _: &KeysetId) -> Result<Vec<PublicKeyInfo>, SealError> {
            // The certificate this client was built from, as a key's id.
            Ok(vec![PublicKeyInfo {
                kid: self.0.clone(),
                public: String::new(),
                created: 0,
            }])
        }
        async fn destroy_keyset(&self, _: &KeysetId) -> Result<(), SealError> {
            unreachable!()
        }
    }

    /// Writes the three files, with `certificate` as the certificate. Each
    /// version is a different length, as a stamp may not tell two writes in
    /// the same instant apart otherwise.
    fn write_certificates(dir: &Path, certificate: &str) {
        std::fs::write(dir.join("ca.pem"), "ca").unwrap();
        std::fs::write(dir.join("tls.key"), "key").unwrap();
        // Renewed as a deployment does it: whole, by a rename.
        std::fs::write(dir.join(".tls.pem.new"), certificate).unwrap();
        std::fs::rename(dir.join(".tls.pem.new"), dir.join("tls.pem")).unwrap();
    }

    /// A client over `dir` that refuses a certificate named `bad…`, and
    /// counts how often one was built.
    fn following(dir: &Path) -> (FollowingCertificates, Arc<std::sync::atomic::AtomicUsize>) {
        let built = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = Arc::clone(&built);
        let client = FollowingCertificates::new(dir.to_path_buf(), move |files| {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if files.cert_pem.starts_with("bad") {
                return Err("the client certificate is not valid".to_owned());
            }
            Ok(Arc::new(Built(files.cert_pem.clone())) as Arc<dyn SealClient>)
        })
        .unwrap();
        (client, built)
    }

    async fn certificate_of(client: &Arc<dyn SealClient>) -> String {
        let keyset = KeysetId::new("any").unwrap();
        client.public_keys(&keyset).await.unwrap().remove(0).kid
    }

    #[tokio::test]
    async fn a_renewed_certificate_is_used_without_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        write_certificates(dir.path(), "first");
        let (client, built) = following(dir.path());
        let before = client.client_after(Duration::ZERO);
        assert_eq!(certificate_of(&before).await, "first");
        // Unchanged files build nothing.
        client.client_after(Duration::ZERO);
        assert_eq!(built.load(std::sync::atomic::Ordering::SeqCst), 1);

        write_certificates(dir.path(), "second, renewed");
        // Not looked at before the interval has passed.
        let soon = client.client_after(Duration::from_secs(3600));
        assert_eq!(certificate_of(&soon).await, "first");
        let after = client.client_after(Duration::ZERO);
        assert_eq!(certificate_of(&after).await, "second, renewed");
        // A call under way keeps the client it started with.
        assert_eq!(certificate_of(&before).await, "first");
        // The trait's calls go to the client of the files as they are.
        let keyset = KeysetId::new("any").unwrap();
        assert_eq!(
            client.public_keys(&keyset).await.unwrap()[0].kid,
            "second, renewed"
        );
    }

    #[tokio::test]
    async fn files_that_do_not_load_keep_the_client_in_use() {
        let dir = tempfile::tempdir().unwrap();
        write_certificates(dir.path(), "first");
        let (client, built) = following(dir.path());

        write_certificates(dir.path(), "bad, half written");
        let kept = client.client_after(Duration::ZERO);
        assert_eq!(certificate_of(&kept).await, "first");
        // The same bad files are tried, and said, once.
        client.client_after(Duration::ZERO);
        assert_eq!(built.load(std::sync::atomic::Ordering::SeqCst), 2);

        // A file that is gone is no client either.
        std::fs::remove_file(dir.path().join("tls.key")).unwrap();
        let kept = client.client_after(Duration::ZERO);
        assert_eq!(certificate_of(&kept).await, "first");

        write_certificates(dir.path(), "third, whole again");
        let renewed = client.client_after(Duration::ZERO);
        assert_eq!(certificate_of(&renewed).await, "third, whole again");
    }

    #[test]
    fn a_server_whose_certificates_do_not_load_is_refused_at_its_start() {
        let dir = tempfile::tempdir().unwrap();
        let mode = SealMode::Mtls {
            addresses: vec!["127.0.0.1:7443".to_owned()],
            tls_dir: dir.path().to_path_buf(),
            server_name: "seal".to_owned(),
        };
        let missing = mode.client().err().unwrap();
        assert!(missing.contains(TLS_DIR_ENV), "{missing}");
        assert!(missing.contains("ca.pem cannot be read"), "{missing}");

        write_certificates(dir.path(), "not a certificate");
        let invalid = mode.client().err().unwrap();
        assert!(invalid.contains(TLS_DIR_ENV), "{invalid}");
    }
}
