// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The deployment's certificates: a CA and one certificate each for the
//! server and the executor, made once (`runtara-executor init-pki`).
//!
//! Each side is written its own key and certificate and the CA's
//! certificate. The CA's key is written nowhere: nothing else can be signed
//! by this CA, and replacing a certificate means making all three again.

use std::path::Path;

use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};

use crate::protocol::{API_NAME, EXECUTOR_NAME, SERVER_NAME};
use crate::tls::{CA_FILE, CERTIFICATE_FILE, KEY_FILE};

/// How long the certificates are good for.
const VALID_DAYS: i64 = 5 * 365;

/// One side's files, as PEM.
pub struct Issued {
    pub ca_pem: String,
    pub certificate_pem: String,
    pub key_pem: String,
}

fn validity(params: &mut CertificateParams) {
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now - time::Duration::days(1);
    params.not_after = now + time::Duration::days(VALID_DAYS);
}

/// A new CA and the server's and the executor's certificates from it.
pub fn issue() -> Result<(Issued, Issued), String> {
    let (server, executor, _) = issue_with_api()?;
    Ok((server, executor))
}

/// A new CA and three certificates from it: the server's, the executor's,
/// and the API role's, for a server whose API and engine are two processes.
/// The server's is then the engine's.
pub fn issue_with_api() -> Result<(Issued, Issued, Issued), String> {
    let failed = |error: rcgen::Error| error.to_string();
    let ca_key = KeyPair::generate().map_err(failed)?;
    let mut ca = CertificateParams::new(Vec::<String>::new()).map_err(failed)?;
    ca.distinguished_name
        .push(DnType::CommonName, "runtara executor CA");
    // It signs the two certificates below and no further CA.
    ca.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    validity(&mut ca);
    let ca_pem = ca.self_signed(&ca_key).map_err(failed)?.pem();
    let issuer = Issuer::from_params(&ca, &ca_key);

    let side = |name: &str, serves: bool| -> Result<Issued, String> {
        let key = KeyPair::generate().map_err(failed)?;
        let mut params = CertificateParams::new(vec![name.to_owned()]).map_err(failed)?;
        params.distinguished_name.push(DnType::CommonName, name);
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        // The server and the executor are each a server to the other's
        // client. The API role only ever calls.
        params.extended_key_usages = if serves {
            vec![
                ExtendedKeyUsagePurpose::ServerAuth,
                ExtendedKeyUsagePurpose::ClientAuth,
            ]
        } else {
            vec![ExtendedKeyUsagePurpose::ClientAuth]
        };
        validity(&mut params);
        let certificate = params.signed_by(&key, &issuer).map_err(failed)?;
        Ok(Issued {
            ca_pem: ca_pem.clone(),
            certificate_pem: certificate.pem(),
            key_pem: key.serialize_pem(),
        })
    };
    Ok((
        side(SERVER_NAME, true)?,
        side(EXECUTOR_NAME, true)?,
        side(API_NAME, false)?,
    ))
}

fn write(directory: &Path, issued: &Issued) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    let put = |file: &str, mode: u32, contents: &str| {
        let path = directory.join(file);
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(mode)
            .open(&path)
            .and_then(|mut file| file.write_all(contents.as_bytes()))
            .map_err(|error| format!("{}: {error}", path.display()))
    };
    put(CA_FILE, 0o644, &issued.ca_pem)?;
    put(CERTIFICATE_FILE, 0o644, &issued.certificate_pem)?;
    // Only its owner reads a key.
    put(KEY_FILE, 0o600, &issued.key_pem)
}

/// Makes the deployment's certificates and writes the server's files to
/// `server`, the executor's to `executor`, and the API role's to `api` if it
/// is given one. Refuses to replace a key that is already there unless
/// `replace`.
pub fn init(
    server: &Path,
    executor: &Path,
    api: Option<&Path>,
    replace: bool,
) -> Result<(), String> {
    if !replace {
        for directory in [Some(server), Some(executor), api].into_iter().flatten() {
            if directory.join(KEY_FILE).exists() {
                return Err(format!(
                    "{} already holds a key; pass --replace to make new certificates for every side",
                    directory.display()
                ));
            }
        }
    }
    let (for_server, for_executor, for_api) = issue_with_api()?;
    write(server, &for_server)?;
    write(executor, &for_executor)?;
    match api {
        Some(api) => write(api, &for_api),
        None => Ok(()),
    }
}
