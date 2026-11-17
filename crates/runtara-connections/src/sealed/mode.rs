// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Where seal runs for this deployment.
//!
//! In this process, the server holds the keys and can open every credential.
//! As a process of its own, on this host over a Unix socket or on another
//! over mutual TLS, seal alone holds them: this process can ask seal to use a
//! credential, toward the destination sealed into it, and cannot read one.

use std::path::PathBuf;
use std::sync::Arc;

use seal_client::{ClientTls, SealClient};

/// `in_process` (the default), `unix` or `mtls`.
pub const MODE_ENV: &str = "RUNTARA_SEAL_MODE";
/// With `unix`: seal's socket. Several, separated by commas, are instances of
/// one seal; a request goes to another only if one cannot be reached.
pub const SOCKET_ENV: &str = "RUNTARA_SEAL_SOCKET";
/// With `mtls`: seal's `host:port`. Several, separated by commas, as above.
pub const ADDRESS_ENV: &str = "RUNTARA_SEAL_ADDRESS";
/// With `mtls`: a directory with `ca.pem` (the CA of seal's certificate),
/// and this server's `tls.pem` and `tls.key`, which seal knows it by.
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
                let read = |file: &str| {
                    std::fs::read_to_string(tls_dir.join(file))
                        .map_err(|error| format!("{TLS_DIR_ENV}: {} cannot be read: {error}", file))
                };
                let tls = ClientTls {
                    ca_pem: read("ca.pem")?,
                    cert_pem: read("tls.pem")?,
                    key_pem: read("tls.key")?,
                    server_name: server_name.clone(),
                };
                seal_client::mtls(addresses, &tls)
                    .map(|client| Some(Arc::new(client) as Arc<dyn SealClient>))
                    .map_err(|error| format!("{TLS_DIR_ENV}: {error}"))
            }
        }
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
}
