// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Mutual TLS between the server and the executor, both ways: the server
//! calls the executor's placement API, the executor calls the server's run
//! channel. Each side has one certificate from the deployment's CA
//! ([`crate::pki`]) and accepts only the other's, by its name: a certificate
//! the CA signed for anything else is refused.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::Full;
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::server::ParsedCertificate;
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, ServerConfig, SignatureScheme};
use sha2::Digest;

/// The CA's certificate.
pub const CA_FILE: &str = "ca.pem";
/// This side's certificate.
pub const CERTIFICATE_FILE: &str = "tls.pem";
/// This side's key.
pub const KEY_FILE: &str = "tls.key";

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn roots(ca_pem: &str) -> Result<RootCertStore, String> {
    let mut roots = RootCertStore::empty();
    for certificate in CertificateDer::pem_slice_iter(ca_pem.as_bytes()) {
        let certificate = certificate.map_err(|_| "the CA is not a PEM certificate".to_owned())?;
        roots
            .add(certificate)
            .map_err(|_| "the CA is not a usable certificate".to_owned())?;
    }
    if roots.is_empty() {
        return Err("the CA holds no certificate".to_owned());
    }
    Ok(roots)
}

/// One side's certificate and key, and the CA both sides trust.
pub struct Identity {
    pub ca_pem: String,
    certificates: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
}

impl Identity {
    /// From `ca.pem`, `tls.pem` and `tls.key` in `directory`. Nothing read is
    /// in an error.
    pub fn load(directory: &Path) -> Result<Self, String> {
        let read = |file: &str| {
            std::fs::read(directory.join(file))
                .map_err(|error| format!("{}: {error}", directory.join(file).display()))
        };
        let ca_pem = String::from_utf8(read(CA_FILE)?)
            .map_err(|_| format!("{CA_FILE} is not a PEM file"))?;
        Self::from_pem(ca_pem, &read(CERTIFICATE_FILE)?, &read(KEY_FILE)?)
    }

    /// From PEM in memory.
    pub fn from_pem(ca_pem: String, certificate: &[u8], key: &[u8]) -> Result<Self, String> {
        roots(&ca_pem)?;
        let certificates = CertificateDer::pem_slice_iter(certificate)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| format!("{CERTIFICATE_FILE} is not a PEM certificate"))?;
        if certificates.is_empty() {
            return Err(format!("{CERTIFICATE_FILE} holds no certificate"));
        }
        let key = PrivateKeyDer::from_pem_slice(key)
            .map_err(|_| format!("{KEY_FILE} is not a PEM private key"))?;
        Ok(Self {
            ca_pem,
            certificates,
            key,
        })
    }

    /// A listener's configuration: this side's certificate, and a client
    /// certificate from the CA required. Whose it is, [`serve`] checks.
    pub fn server_config(&self) -> Result<ServerConfig, String> {
        let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
            Arc::new(roots(&self.ca_pem)?),
            provider(),
        )
        .build()
        .map_err(|error| error.to_string())?;
        let mut config = ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .map_err(|error| error.to_string())?
            .with_client_cert_verifier(verifier)
            .with_single_cert(self.certificates.clone(), self.key.clone_key())
            .map_err(|error| error.to_string())?;
        config.alpn_protocols = vec![b"h2".to_vec()];
        Ok(config)
    }

    /// A client's configuration: this side's certificate presented, and the
    /// peer's verified against `ca_pem` alone (no system roots) and, when
    /// given, the SHA-256 `pin` of its public key.
    pub fn client_config(&self, ca_pem: &str, pin: Option<&str>) -> Result<ClientConfig, String> {
        let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(
            Arc::new(roots(ca_pem)?),
            provider(),
        )
        .build()
        .map_err(|error| error.to_string())?;
        let verifier: Arc<dyn ServerCertVerifier> = match pin {
            None => verifier,
            Some(pin) => {
                let pin = decode_pin(pin).ok_or("the pin is not a SHA-256 in hex")?;
                Arc::new(Pinned {
                    chain: verifier,
                    pin,
                })
            }
        };
        ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .map_err(|error| error.to_string())?
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_client_auth_cert(self.certificates.clone(), self.key.clone_key())
            .map_err(|error| error.to_string())
    }
}

fn decode_pin(pin: &str) -> Option<[u8; 32]> {
    if pin.len() != 64 || !pin.is_ascii() {
        return None;
    }
    let mut decoded = [0u8; 32];
    for (index, byte) in decoded.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&pin[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(decoded)
}

/// The usual verification, and then the certificate's public key must be the
/// pinned one.
#[derive(Debug)]
struct Pinned {
    chain: Arc<dyn ServerCertVerifier>,
    pin: [u8; 32],
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let verified = self.chain.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            ocsp_response,
            now,
        )?;
        let key = ParsedCertificate::try_from(end_entity)?.subject_public_key_info();
        if sha2::Sha256::digest(key.as_ref()).as_slice() != self.pin {
            return Err(rustls::Error::General(
                "the certificate's key is not the pinned one".to_owned(),
            ));
        }
        Ok(verified)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.chain.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.chain.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.chain.supported_verify_schemes()
    }
}

/// Whether the peer's certificate is for `name` exactly.
fn peer_is(certificates: Option<&[CertificateDer<'static>]>, name: &str) -> bool {
    let Some(certificate) = certificates.and_then(|chain| chain.first()) else {
        return false;
    };
    let (Ok(certificate), Ok(name)) = (
        ParsedCertificate::try_from(certificate),
        ServerName::try_from(name),
    ) else {
        return false;
    };
    rustls::client::verify_server_name(&certificate, &name).is_ok()
}

/// Serves `router` on `listener` over mutual TLS to the peer whose
/// certificate is for `peer`; anyone else's connection is closed after the
/// handshake. Runs until dropped.
pub async fn serve(
    listener: tokio::net::TcpListener,
    config: ServerConfig,
    peer: &'static str,
    router: axum::Router,
) {
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    loop {
        let (stream, from) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                tracing::warn!(%error, "accepting a connection failed");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            }
        };
        let (acceptor, router) = (acceptor.clone(), router.clone());
        tokio::spawn(async move {
            let Ok(stream) = acceptor.accept(stream).await else {
                tracing::debug!(%from, "a TLS handshake failed");
                return;
            };
            if !peer_is(stream.get_ref().1.peer_certificates(), peer) {
                tracing::warn!(%from, peer, "refused a certificate that is not the peer's");
                return;
            }
            let service = hyper_util::service::TowerToHyperService::new(router);
            let _ = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
    }
}

/// An HTTP/2 client over mutual TLS.
pub type Client = hyper_util::client::legacy::Client<
    hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
    Full<Bytes>,
>;

/// A client that verifies its peer's certificate against `server_name`,
/// whatever host it connects to.
pub fn client(config: ClientConfig, server_name: &str) -> Result<Client, String> {
    let name = ServerName::try_from(server_name.to_owned())
        .map_err(|_| format!("{server_name} is not a name a certificate can be for"))?;
    let connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_tls_config(config)
        .https_only()
        .with_server_name_resolver(hyper_rustls::FixedServerNameResolver::new(name))
        .enable_http2()
        .build();
    Ok(hyper_util::client::legacy::Client::builder(TokioExecutor::new()).build(connector))
}

/// Binds `address`.
pub async fn bind(address: SocketAddr) -> Result<tokio::net::TcpListener, String> {
    tokio::net::TcpListener::bind(address)
        .await
        .map_err(|error| format!("{address}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pin_is_sixty_four_hex_digits() {
        assert_eq!(decode_pin(&"ab".repeat(32)), Some([0xab; 32]));
        assert_eq!(decode_pin("ab"), None);
        assert_eq!(decode_pin(&"zz".repeat(32)), None);
    }
}
