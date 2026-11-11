//! How a pool reaches a tenant's Postgres database: through seal.
//!
//! A `postgres` connection's password is sealed. The pool connects to a Unix
//! socket this process owns, with no TLS and no password, and each session
//! is seal's from there: seal opens it as the connection's policy says
//! (verifying the server where the URL asked for that), logs in with the
//! sealed password, and relays the session. seal makes the address checks
//! itself: a tenant's connection cannot reach the server's own network
//! (loopback, private ranges, the cloud metadata service) unless the
//! deployment allows it (`RUNTARA_DATABASE_CONNECTION_GUARD` off, or a host
//! on `RUNTARA_PROXY_ALLOWED_HOSTS`).

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Mutex, OnceLock};

/// The URL to connect with for a Postgres connection whose password is
/// sealed, or why it is refused.
///
/// The pool connects to a Unix socket this process owns, with no TLS and no
/// password, as the connection's user and database. Each session is seal's
/// from there: seal verifies the server, logs in with the sealed password,
/// and relays the session. seal makes the address checks itself, refusing a
/// private address its deployment does not allow.
pub(crate) async fn admit_sealed(
    sealed: std::sync::Arc<runtara_connections::sealed::SealedCredentials>,
    tenant: &str,
    connection_id: &str,
    parameters: &serde_json::Value,
) -> Result<String, String> {
    use runtara_connections::sealed::policy::{PG_TARGET_FIELD, postgres_target};
    let target = postgres_target(parameters)
        .ok_or_else(|| "the connection names no database".to_string())?;
    let directory = relay::to_seal(
        sealed,
        tenant.to_owned(),
        connection_id.to_owned(),
        parameters.clone(),
        target.port,
    )
    .await?;
    let mut url = url::Url::parse(parameters[PG_TARGET_FIELD].as_str().unwrap_or_default())
        .map_err(|_| "the connection's database target does not parse".to_string())?;
    // The pool speaks to the relay in the clear, whatever the session to the
    // server is: that leg is seal's.
    url.set_query(None);
    url.query_pairs_mut()
        .append_pair("sslmode", "disable")
        .append_pair("host", &directory);
    let relayed = url.to_string();
    // It must reach the relay, and nothing else.
    let dialed = sqlx::postgres::PgConnectOptions::from_str(&relayed)
        .map_err(|_| "the relayed database URL does not parse".to_string())?;
    if dialed.get_socket().and_then(|socket| socket.to_str()) != Some(directory.as_str()) {
        return Err("the database URL does not dial seal's relay".to_string());
    }
    Ok(relayed)
}

/// Relays from Unix sockets this process owns to sessions seal opens.
mod relay {
    use super::*;

    /// The most addresses one tenant's connections bring a relay to, so a
    /// tenant cannot use up the process's relays.
    pub(super) const RELAYS_PER_TENANT: usize = 16;
    /// The most addresses one process relays to.
    pub(super) const MAX_RELAYS: usize = 4096;

    /// The relays to seal of this process, by what they log in with.
    #[derive(Default)]
    struct SealRelays {
        directories: HashMap<String, (String, tokio::task::JoinHandle<()>)>,
        by_tenant: HashMap<String, std::collections::HashSet<String>>,
    }

    fn seal_relays() -> &'static Mutex<SealRelays> {
        static RELAYS: OnceLock<Mutex<SealRelays>> = OnceLock::new();
        RELAYS.get_or_init(Mutex::default)
    }

    /// The directory of the relay to seal for a sealed Postgres connection,
    /// started on first use. sqlx connects to `{directory}/.s.PGSQL.{port}`,
    /// and each connection becomes a session seal opens with the sealed
    /// password. A connection whose sealed password or target changes gets a
    /// relay of its own.
    #[cfg(unix)]
    pub(super) async fn to_seal(
        sealed: std::sync::Arc<runtara_connections::sealed::SealedCredentials>,
        tenant: String,
        connection_id: String,
        parameters: serde_json::Value,
        port: u16,
    ) -> Result<String, String> {
        use sha2::{Digest, Sha256};
        use std::os::unix::fs::DirBuilderExt;

        let sealed_as = [
            runtara_connections::sealed::policy::PG_URL_FIELD,
            runtara_connections::sealed::policy::PG_TARGET_FIELD,
        ]
        .map(|field| parameters[field].to_string());
        let key = hex::encode(Sha256::digest(
            format!(
                "{tenant}\n{connection_id}\n{}\n{}",
                sealed_as[0], sealed_as[1]
            )
            .as_bytes(),
        ));

        let mut relays = seal_relays().lock().expect("database relays poisoned");
        let relays = &mut *relays;
        if relays
            .directories
            .get(&key)
            .is_some_and(|(_, task)| task.is_finished())
            && let Some((directory, _)) = relays.directories.remove(&key)
        {
            let _ = std::fs::remove_dir_all(directory);
        }
        if let Some((directory, _)) = relays.directories.get(&key) {
            return Ok(directory.clone());
        }
        let mine = relays.by_tenant.entry(tenant.clone()).or_default();
        if mine.len() >= RELAYS_PER_TENANT {
            return Err(format!(
                "this tenant's connections already reach {RELAYS_PER_TENANT} sealed databases"
            ));
        }
        if relays.directories.len() >= MAX_RELAYS {
            return Err(format!(
                "this server already relays to {MAX_RELAYS} sealed databases"
            ));
        }
        let directory = std::env::temp_dir().join(format!(
            "rtdb-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..12]
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .map_err(|error| format!("cannot create a database relay: {error}"))?;
        let socket = directory.join(format!(".s.PGSQL.{port}"));
        if socket.as_os_str().len() >= 104 {
            return Err("the database relay's socket path is too long".to_string());
        }
        let listener = tokio::net::UnixListener::bind(&socket)
            .map_err(|error| format!("cannot start a database relay: {error}"))?;
        let task = tokio::spawn(serve_seal(
            listener,
            sealed,
            tenant,
            connection_id,
            parameters,
        ));
        let directory = directory
            .to_str()
            .ok_or("the database relay's directory is not UTF-8")?
            .to_owned();
        relays
            .directories
            .insert(key.clone(), (directory.clone(), task));
        mine.insert(key);
        Ok(directory)
    }

    /// No Unix sockets: nothing for a pool to reach seal through.
    #[cfg(not(unix))]
    pub(super) async fn to_seal(
        _: std::sync::Arc<runtara_connections::sealed::SealedCredentials>,
        _: String,
        _: String,
        _: serde_json::Value,
        _: u16,
    ) -> Result<String, String> {
        Err("a sealed database connection needs Unix sockets".to_string())
    }

    /// Hands each connection to a session seal opens for the sealed
    /// connection. A login seal refuses reaches the pool as a Postgres error.
    #[cfg(unix)]
    async fn serve_seal(
        listener: tokio::net::UnixListener,
        sealed: std::sync::Arc<runtara_connections::sealed::SealedCredentials>,
        tenant: String,
        connection_id: String,
        parameters: serde_json::Value,
    ) {
        loop {
            let mut client = match listener.accept().await {
                Ok((client, _)) => client,
                Err(error) => {
                    tracing::warn!(%connection_id, %error, "sealed database relay could not accept");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                }
            };
            let (sealed, tenant, connection_id, parameters) = (
                sealed.clone(),
                tenant.clone(),
                connection_id.clone(),
                parameters.clone(),
            );
            tokio::spawn(async move {
                match sealed
                    .connect_pg(&tenant, &connection_id, &parameters)
                    .await
                {
                    Ok(mut session) => {
                        let _ = tokio::io::copy_bidirectional(&mut client, &mut session).await;
                    }
                    // seal's detail has identifiers and static text only.
                    Err(error) => {
                        tracing::warn!(
                            %connection_id,
                            code = error.code.as_str(),
                            detail = %error.detail,
                            "seal did not open the database session"
                        );
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes the server's certificate and key where Postgres may read them,
    /// and starts it with TLS on.
    const POSTGRES_WITH_TLS: &str = r#"set -e
mkdir -p /certs
printf '%s' "$SERVER_CERT" > /certs/server.crt
printf '%s' "$SERVER_KEY" > /certs/server.key
chown -R postgres:postgres /certs
chmod 600 /certs/server.key
exec docker-entrypoint.sh postgres -c ssl=on -c ssl_cert_file=/certs/server.crt \
  -c ssl_key_file=/certs/server.key
"#;

    /// A sealed Postgres connection's pool reaches a real database through
    /// seal: seal verifies the server against this deployment's roots, logs
    /// in with the sealed password, and relays the session. The URL the pool
    /// is given holds no password.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_sealed_database_connection_logs_in_through_seal() {
        use rcgen::{
            BasicConstraints, CertificateParams, CertifiedIssuer, ExtendedKeyUsagePurpose, IsCa,
            KeyPair, KeyUsagePurpose,
        };
        use runtara_connections::sealed::{Allowances, MemoryKeySource, SealedCredentials};
        use sqlx::{Connection, Row};
        use testcontainers::core::{IntoContainerPort, WaitFor};
        use testcontainers::runners::AsyncRunner;
        use testcontainers::{GenericImage, ImageExt};

        let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyCertSign,
        ];
        let ca = CertifiedIssuer::self_signed(ca, KeyPair::generate().unwrap()).unwrap();
        let mut server = CertificateParams::new(vec!["127.0.0.1".to_string()]).unwrap();
        server.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let server_key = KeyPair::generate().unwrap();
        let server_cert = server.signed_by(&server_key, &ca).unwrap();

        let container = GenericImage::new("postgres", "16-alpine")
            .with_exposed_port(5432.tcp())
            .with_wait_for(WaitFor::message_on_stderr(
                "database system is ready to accept connections",
            ))
            .with_entrypoint("sh")
            .with_cmd(["-c", POSTGRES_WITH_TLS])
            .with_env_var("POSTGRES_USER", "app")
            .with_env_var("POSTGRES_PASSWORD", "synthetic-database-password")
            .with_env_var("POSTGRES_DB", "orders")
            .with_env_var("SERVER_CERT", server_cert.pem())
            .with_env_var("SERVER_KEY", server_key.serialize_pem())
            .start()
            .await
            .expect("required Docker Postgres container must start");
        let port = container.get_host_port_ipv4(5432).await.unwrap();

        let sealed = std::sync::Arc::new(
            SealedCredentials::in_process(
                std::sync::Arc::new(MemoryKeySource::default()),
                &[7_u8; 32],
                Allowances {
                    private_hosts: vec![format!("127.0.0.1:{port}")],
                    // The test's CA, as a deployment would trust its own.
                    extra_roots: vec![ca.pem()],
                    ..Default::default()
                },
            )
            .unwrap(),
        );
        let seal = |password: &'static str, connection: &'static str| {
            let sealed = sealed.clone();
            async move {
                let entered = serde_json::json!({
                    "database_url": format!(
                        "postgres://app:{password}@127.0.0.1:{port}/orders?sslmode=verify-full"
                    ),
                });
                let stored = sealed
                    .seal_parameters("acme", connection, "postgres", &entered, None)
                    .await
                    .unwrap()
                    .expect("a verified Postgres connection is sealed");
                assert!(!stored.to_string().contains(password));
                stored
            }
        };

        let stored = seal("synthetic-database-password", "c-db").await;
        let url = admit_sealed(sealed.clone(), "acme", "c-db", &stored)
            .await
            .unwrap();
        assert!(!url.contains("synthetic-database-password"), "{url}");
        assert!(url.contains("sslmode=disable"), "{url}");
        // The same connection keeps its relay.
        assert_eq!(
            admit_sealed(sealed.clone(), "acme", "c-db", &stored)
                .await
                .unwrap(),
            url
        );

        // The image starts Postgres once to set it up, then again to serve:
        // a first connection may arrive in between.
        let mut connection = None;
        for _ in 0..50 {
            match sqlx::postgres::PgConnection::connect(&url).await {
                Ok(open) => {
                    connection = Some(open);
                    break;
                }
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(200)).await,
            }
        }
        let mut connection = connection.expect("seal logs in to the database");
        let row = sqlx::query("SELECT current_user::text, current_database()::text, ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()")
            .fetch_one(&mut connection)
            .await
            .unwrap();
        assert_eq!(row.get::<String, _>(0), "app");
        assert_eq!(row.get::<String, _>(1), "orders");
        // The session to the server is over TLS, though the pool asked for none.
        assert!(row.get::<bool, _>(2));
        // A pool works through it too.
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .unwrap();
        let sum: i32 = sqlx::query_scalar("SELECT 40 + 2")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(sum, 42);

        // A URL that asks for TLS without verifying the server (`require`) is
        // sealed for that: seal, trusting no CA of this test's, still logs in.
        if runtara_connections::sealed::policy::seal_logs_in_unverified() {
            let trusting_nothing = std::sync::Arc::new(
                SealedCredentials::in_process(
                    std::sync::Arc::new(MemoryKeySource::default()),
                    &[7_u8; 32],
                    Allowances {
                        private_hosts: vec![format!("127.0.0.1:{port}")],
                        ..Default::default()
                    },
                )
                .unwrap(),
            );
            let entered = serde_json::json!({
                "database_url": format!(
                    "postgres://app:synthetic-database-password@127.0.0.1:{port}/orders?sslmode=require"
                ),
            });
            let stored = trusting_nothing
                .seal_parameters("acme", "c-require", "postgres", &entered, None)
                .await
                .unwrap()
                .expect("a URL that asks for unverified TLS is sealed for it");
            let unverified_url = admit_sealed(trusting_nothing, "acme", "c-require", &stored)
                .await
                .unwrap();
            let mut unverified = sqlx::postgres::PgConnection::connect(&unverified_url)
                .await
                .expect("seal logs in without verifying the server");
            let encrypted: bool =
                sqlx::query_scalar("SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()")
                    .fetch_one(&mut unverified)
                    .await
                    .unwrap();
            assert!(encrypted);
        }

        // A wrong password is the server's to refuse, and reaches the pool as
        // a failed connection.
        let wrong = seal("not-the-password", "c-wrong").await;
        let wrong_url = admit_sealed(sealed.clone(), "acme", "c-wrong", &wrong)
            .await
            .unwrap();
        assert_ne!(wrong_url, url);
        assert!(
            sqlx::postgres::PgConnection::connect(&wrong_url)
                .await
                .is_err()
        );

        // Another database than the one the password was sealed for is seal's
        // to refuse, with the right password or not.
        let elsewhere = url.replace("/orders?", "/postgres?");
        assert!(
            sqlx::postgres::PgConnection::connect(&elsewhere)
                .await
                .is_err()
        );
    }

    /// A URL that does not ask for a verified server is sealed for what it
    /// asks: `sslmode=disable` reaches a database with no TLS, and seal logs
    /// in there with SCRAM, so the password is never sent.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_sealed_connection_to_a_database_without_tls_logs_in_through_seal() {
        use runtara_connections::sealed::{Allowances, MemoryKeySource, SealedCredentials, policy};
        use sqlx::{Connection, Row};
        use testcontainers::core::{IntoContainerPort, WaitFor};
        use testcontainers::runners::AsyncRunner;
        use testcontainers::{GenericImage, ImageExt};

        // seal before it could log in without TLS leaves such a connection
        // readable: there is then nothing sealed to check.
        if !policy::seal_logs_in_unverified() {
            eprintln!("skipped: this build's seal seals only verified Postgres connections");
            return;
        }
        // As the image comes: no TLS, and SCRAM for every host.
        let container = GenericImage::new("postgres", "16-alpine")
            .with_exposed_port(5432.tcp())
            .with_wait_for(WaitFor::message_on_stderr(
                "database system is ready to accept connections",
            ))
            .with_env_var("POSTGRES_USER", "app")
            .with_env_var("POSTGRES_PASSWORD", "synthetic-database-password")
            .with_env_var("POSTGRES_DB", "orders")
            .start()
            .await
            .expect("required Docker Postgres container must start");
        let port = container.get_host_port_ipv4(5432).await.unwrap();

        let sealed = std::sync::Arc::new(
            SealedCredentials::in_process(
                std::sync::Arc::new(MemoryKeySource::default()),
                &[7_u8; 32],
                // As a deployment whose address guard for databases is off,
                // and that lets this host be reached in the clear.
                Allowances {
                    private_databases: true,
                    plain_http_hosts: vec!["127.0.0.1".into()],
                    ..Default::default()
                },
            )
            .unwrap(),
        );
        {
            let (mode, connection) = ("disable", "c-plain");
            let entered = serde_json::json!({
                "database_url": format!(
                    "postgres://app:synthetic-database-password@127.0.0.1:{port}/orders?sslmode={mode}"
                ),
            });
            let stored = sealed
                .seal_parameters("acme", connection, "postgres", &entered, None)
                .await
                .unwrap()
                .expect("the connection is sealed for the protection its URL asks for");
            assert!(!stored.to_string().contains("synthetic-database-password"));
            let url = admit_sealed(sealed.clone(), "acme", connection, &stored)
                .await
                .unwrap();
            assert!(!url.contains("synthetic-database-password"), "{url}");

            let mut opened = None;
            for _ in 0..50 {
                match sqlx::postgres::PgConnection::connect(&url).await {
                    Ok(open) => {
                        opened = Some(open);
                        break;
                    }
                    Err(_) => tokio::time::sleep(std::time::Duration::from_millis(200)).await,
                }
            }
            let mut opened = opened.expect("seal logs in to the database");
            let row = sqlx::query(
                "SELECT current_user::text, ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
            )
            .fetch_one(&mut opened)
            .await
            .unwrap();
            assert_eq!(row.get::<String, _>(0), "app", "{mode}");
            // The server has no TLS, so the session has none.
            assert!(!row.get::<bool, _>(1), "{mode}");
        }

        // A URL that asks for TLS is not given a session without it.
        let required = serde_json::json!({
            "database_url": format!(
                "postgres://app:synthetic-database-password@127.0.0.1:{port}/orders?sslmode=require"
            ),
        });
        let stored = sealed
            .seal_parameters("acme", "c-require", "postgres", &required, None)
            .await
            .unwrap()
            .unwrap();
        let url = admit_sealed(sealed.clone(), "acme", "c-require", &stored)
            .await
            .unwrap();
        assert!(sqlx::postgres::PgConnection::connect(&url).await.is_err());
    }
}
