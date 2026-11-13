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

use crate::protocol::{EXECUTOR_NAME, SERVER_NAME};
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

    let side = |name: &str| -> Result<Issued, String> {
        let key = KeyPair::generate().map_err(failed)?;
        let mut params = CertificateParams::new(vec![name.to_owned()]).map_err(failed)?;
        params.distinguished_name.push(DnType::CommonName, name);
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        // Each side is a server to the other's client.
        params.extended_key_usages = vec![
            ExtendedKeyUsagePurpose::ServerAuth,
            ExtendedKeyUsagePurpose::ClientAuth,
        ];
        validity(&mut params);
        let certificate = params.signed_by(&key, &issuer).map_err(failed)?;
        Ok(Issued {
            ca_pem: ca_pem.clone(),
            certificate_pem: certificate.pem(),
            key_pem: key.serialize_pem(),
        })
    };
    Ok((side(SERVER_NAME)?, side(EXECUTOR_NAME)?))
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
/// `server` and the executor's to `executor`. Refuses to replace a key that
/// is already there unless `replace`.
pub fn init(server: &Path, executor: &Path, replace: bool) -> Result<(), String> {
    if !replace {
        for directory in [server, executor] {
            if directory.join(KEY_FILE).exists() {
                return Err(format!(
                    "{} already holds a key; pass --replace to make new certificates for both sides",
                    directory.display()
                ));
            }
        }
    }
    let (for_server, for_executor) = issue()?;
    write(server, &for_server)?;
    write(executor, &for_executor)
}
