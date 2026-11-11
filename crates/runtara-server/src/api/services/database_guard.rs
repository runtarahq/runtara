//! The address guard for tenant database connections: the database
//! counterpart of the HTTP egress guard (`runtara_connections::net`).
//!
//! A tenant's Postgres connection names a host; without a guard it could
//! reach the server's own network: loopback, private ranges, the cloud
//! metadata service. When the guard is enforced (by default on a server
//! without a platform database, `RUNTARA_DATABASE_CONNECTION_GUARD`), every
//! address the host resolves to must be public, unless the host is on the
//! egress allowlist (`RUNTARA_PROXY_ALLOWED_HOSTS`). The vetted address is
//! the dialed one: the URL is pinned to it, so a host cannot resolve to a
//! public address for the check and a private one for the connect.
//!
//! A `sslmode=verify-full` URL must keep its host name, which certificate
//! verification checks, and sqlx dials the host it verifies. So such a
//! connection goes through a relay instead: sqlx connects to a Unix socket
//! this process owns (the URL's `host=` directory, its TLS name unchanged),
//! and the relay dials the vetted address, never resolving the name again.
//! Every connection of the pool, reconnects included, goes the same way.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::sync::{Mutex, OnceLock};

use runtara_connections::net::{host_is_allowlisted_for_egress, is_private_ip};

/// The URL to connect with for `database_url`, or why it is refused.
/// `tenant` is whose connection it is: relays are counted per tenant.
pub(crate) async fn admit(database_url: &str, tenant: &str) -> Result<String, String> {
    if !crate::config::database_connection_guard() {
        return Ok(database_url.to_owned());
    }
    // The platform's own database is the operator's, not a tenant's.
    if crate::config::object_model_database_url().as_deref() == Some(database_url) {
        return Ok(database_url.to_owned());
    }
    admit_with(database_url, tenant, host_is_allowlisted_for_egress).await
}

async fn admit_with(
    database_url: &str,
    tenant: &str,
    allowlisted: impl Fn(&str) -> bool,
) -> Result<String, String> {
    // Everything is read through the connector's own parser, so the guard
    // checks the target the connection will actually dial.
    let options = sqlx::postgres::PgConnectOptions::from_str(database_url)
        .map_err(|_| "the database URL does not parse".to_string())?;
    // The authority is the only place a target may be named: the `host` and
    // `hostaddr` parameters override it in the connector.
    if let Ok(url) = url::Url::parse(database_url)
        && url.query_pairs().any(|(key, _)| {
            key.eq_ignore_ascii_case("host") || key.eq_ignore_ascii_case("hostaddr")
        })
    {
        return Err(
            "name the database host in the URL, not in a host or hostaddr parameter".to_string(),
        );
    }
    if options.get_socket().is_some() {
        return Err("a database connection cannot use a local socket".to_string());
    }
    let host = options.get_host().to_owned();
    if allowlisted(&host) {
        return Ok(database_url.to_owned());
    }
    let literal = host.trim_start_matches('[').trim_end_matches(']');
    let addresses: Vec<IpAddr> = match literal.parse::<IpAddr>() {
        Ok(ip) => vec![ip],
        Err(_) => tokio::net::lookup_host((host.as_str(), options.get_port()))
            .await
            .map_err(|error| format!("host '{host}' does not resolve: {error}"))?
            .map(|address| address.ip())
            .collect(),
    };
    if addresses.is_empty() {
        return Err(format!("host '{host}' does not resolve"));
    }
    if let Some(private) = addresses.iter().find(|ip| is_private_ip(ip)) {
        return Err(format!(
            "host '{host}' resolves to {private}, a private, loopback or metadata address; \
             database connections must reach public addresses"
        ));
    }
    let address = addresses[0];
    // Certificate verification needs the host name: dialed through a relay.
    if matches!(
        options.get_ssl_mode(),
        sqlx::postgres::PgSslMode::VerifyFull
    ) {
        let directory = relay::to(SocketAddr::new(address, options.get_port()), tenant).await?;
        return through_relay(database_url, &host, &directory);
    }
    let pinned = pin(database_url, address)?;
    // The pinned URL must dial exactly the vetted address.
    let dialed = sqlx::postgres::PgConnectOptions::from_str(&pinned)
        .map_err(|_| "the pinned database URL does not parse".to_string())?;
    let dialed_host = dialed
        .get_host()
        .trim_start_matches('[')
        .trim_end_matches(']');
    if dialed.get_socket().is_some() || dialed_host.parse::<IpAddr>().ok() != Some(address) {
        return Err("the database URL does not dial the vetted address".to_string());
    }
    Ok(pinned)
}

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

/// `database_url` connecting through the relay in `directory`, with its host
/// name kept for certificate verification.
fn through_relay(database_url: &str, host: &str, directory: &str) -> Result<String, String> {
    let mut url =
        url::Url::parse(database_url).map_err(|_| "the database URL does not parse".to_string())?;
    url.query_pairs_mut().append_pair("host", directory);
    let relayed = url.to_string();
    // It must reach the relay, and verify the same host name.
    let dialed = sqlx::postgres::PgConnectOptions::from_str(&relayed)
        .map_err(|_| "the relayed database URL does not parse".to_string())?;
    if dialed.get_socket().and_then(|socket| socket.to_str()) != Some(directory)
        || dialed.get_host() != host
    {
        return Err("the database URL does not dial the vetted address".to_string());
    }
    Ok(relayed)
}

/// Relays from Unix sockets this process owns to vetted database addresses.
mod relay {
    use super::*;

    /// The most addresses one tenant's connections bring a relay to, so a
    /// tenant cannot use up the process's relays.
    pub(super) const RELAYS_PER_TENANT: usize = 16;
    /// The most addresses one process relays to.
    pub(super) const MAX_RELAYS: usize = 4096;

    /// The relays of this process. A relay stays while the process runs: a
    /// pool reconnects through its socket.
    #[derive(Default)]
    struct Relays {
        /// The relay directory for each vetted address, and the task serving
        /// it.
        directories: HashMap<SocketAddr, (String, tokio::task::JoinHandle<()>)>,
        /// The addresses each tenant brought a relay to.
        by_tenant: HashMap<String, std::collections::HashSet<SocketAddr>>,
    }

    fn relays() -> &'static Mutex<Relays> {
        static RELAYS: OnceLock<Mutex<Relays>> = OnceLock::new();
        RELAYS.get_or_init(Mutex::default)
    }

    /// The directory of the relay to `address`, started on first use. sqlx
    /// connects to `{directory}/.s.PGSQL.{port}`.
    #[cfg(unix)]
    pub(super) async fn to(address: SocketAddr, tenant: &str) -> Result<String, String> {
        use std::os::unix::fs::DirBuilderExt;

        let mut relays = relays().lock().expect("database relays poisoned");
        let relays = &mut *relays;
        // A relay whose task has ended (its runtime gone, as between tests)
        // serves nothing: it is started again rather than handed out.
        if relays
            .directories
            .get(&address)
            .is_some_and(|(_, task)| task.is_finished())
            && let Some((directory, _)) = relays.directories.remove(&address)
        {
            let _ = std::fs::remove_dir_all(directory);
        }
        let mine = relays.by_tenant.entry(tenant.to_owned()).or_default();
        if let Some((directory, _)) = relays.directories.get(&address) {
            // Another tenant's relay to the same address is shared, and
            // counts for this tenant too.
            if mine.contains(&address) || mine.len() < RELAYS_PER_TENANT {
                mine.insert(address);
                return Ok(directory.clone());
            }
        }
        if !mine.contains(&address) && mine.len() >= RELAYS_PER_TENANT {
            return Err(format!(
                "this tenant's connections already reach {RELAYS_PER_TENANT} \
                 sslmode=verify-full database addresses"
            ));
        }
        if relays.directories.len() >= MAX_RELAYS {
            return Err(format!(
                "this server already relays to {MAX_RELAYS} database addresses"
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
        let socket = directory.join(format!(".s.PGSQL.{}", address.port()));
        // sockaddr_un holds 104 bytes on macOS, 108 on Linux.
        if socket.as_os_str().len() >= 104 {
            return Err("the database relay's socket path is too long".to_string());
        }
        let listener = tokio::net::UnixListener::bind(&socket)
            .map_err(|error| format!("cannot start a database relay: {error}"))?;
        let task = tokio::spawn(serve(listener, address));
        let directory = directory
            .to_str()
            .ok_or("the database relay's directory is not UTF-8")?
            .to_owned();
        relays
            .directories
            .insert(address, (directory.clone(), task));
        mine.insert(address);
        Ok(directory)
    }

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

    /// No Unix sockets: a host name cannot be both verified and pinned.
    #[cfg(not(unix))]
    pub(super) async fn to(_: SocketAddr, _: &str) -> Result<String, String> {
        Err("sslmode=verify-full needs an allowlisted host on this platform".to_string())
    }

    /// Pipes each connection to `address`, the one vetted at admission.
    #[cfg(unix)]
    pub(super) async fn serve(listener: tokio::net::UnixListener, address: SocketAddr) {
        loop {
            let mut client = match listener.accept().await {
                Ok((client, _)) => client,
                Err(error) => {
                    // Out of descriptors, say: wait rather than spin.
                    tracing::warn!(%address, %error, "database relay could not accept");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                }
            };
            tokio::spawn(async move {
                match tokio::net::TcpStream::connect(address).await {
                    Ok(mut database) => {
                        let _ = tokio::io::copy_bidirectional(&mut client, &mut database).await;
                    }
                    Err(error) => {
                        tracing::warn!(%address, %error, "database relay could not dial");
                    }
                }
            });
        }
    }
}

/// `database_url` with its host replaced by the vetted address.
fn pin(database_url: &str, address: IpAddr) -> Result<String, String> {
    let mut url =
        url::Url::parse(database_url).map_err(|_| "the database URL does not parse".to_string())?;
    if url.host_str().is_none() {
        return Err("the database URL names no host".to_string());
    }
    url.set_ip_host(address)
        .map_err(|_| "the database URL cannot take an address".to_string())?;
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nothing_allowlisted(_: &str) -> bool {
        false
    }

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

    #[tokio::test]
    async fn private_loopback_and_metadata_addresses_are_refused() {
        for url in [
            "postgres://u:p@127.0.0.1:5432/db",
            "postgres://u:p@localhost/db",
            "postgres://u:p@10.0.0.5/db",
            "postgres://u:p@192.168.1.10/db",
            "postgres://u:p@169.254.169.254/db",
            "postgres://u:p@[::1]/db",
            "postgres://u:p@[::ffff:127.0.0.1]/db",
            "postgres://u:p@8.8.8.8/db?host=/var/run/postgresql",
            "postgres://u:p@8.8.8.8/db?host=127.0.0.1",
            "postgres://u:p@8.8.8.8/db?hostaddr=169.254.169.254",
            "postgres://u:p@8.8.8.8/db?HOST=10.0.0.1",
        ] {
            assert!(
                admit_with(url, "t", nothing_allowlisted).await.is_err(),
                "{url}"
            );
        }
    }

    #[tokio::test]
    async fn a_public_address_is_admitted_and_pinned() {
        let admitted = admit_with(
            "postgres://u:p@8.8.8.8:5433/db?sslmode=require",
            "t",
            nothing_allowlisted,
        )
        .await
        .unwrap();
        assert_eq!(admitted, "postgres://u:p@8.8.8.8:5433/db?sslmode=require");
    }

    /// Host-name verification keeps the host name and dials the vetted
    /// address through a relay: the connector reaches the relay's socket,
    /// and certificate verification still sees the URL's host.
    #[tokio::test]
    async fn a_verified_connection_goes_through_a_relay_to_the_vetted_address() {
        let verified = "postgres://u:p@8.8.8.8:5433/db?sslmode=verify-full";
        let admitted = admit_with(verified, "t", nothing_allowlisted)
            .await
            .unwrap();
        let options = sqlx::postgres::PgConnectOptions::from_str(&admitted).unwrap();
        assert_eq!(options.get_host(), "8.8.8.8");
        assert_eq!(options.get_port(), 5433);
        assert!(matches!(
            options.get_ssl_mode(),
            sqlx::postgres::PgSslMode::VerifyFull
        ));
        let directory = options.get_socket().unwrap().to_owned();
        assert!(directory.join(".s.PGSQL.5433").exists(), "{directory:?}");
        // One relay per vetted address.
        assert_eq!(
            admit_with(verified, "t", nothing_allowlisted)
                .await
                .unwrap(),
            admitted
        );
    }

    /// A host name in the URL is kept for verification beside the relay.
    #[test]
    fn a_relayed_url_keeps_its_host_name() {
        let relayed = through_relay(
            "postgres://u:p@db.example.com:6543/app?sslmode=verify-full",
            "db.example.com",
            "/tmp/rtdb-x",
        )
        .unwrap();
        let options = sqlx::postgres::PgConnectOptions::from_str(&relayed).unwrap();
        assert_eq!(options.get_host(), "db.example.com");
        assert_eq!(
            options.get_socket().unwrap(),
            std::path::Path::new("/tmp/rtdb-x")
        );
    }

    /// Through the relay, certificate verification still checks the URL's
    /// host name. Needs a TLS PostgreSQL whose certificate names `db.test`:
    /// `DATABASE_GUARD_TLS_ADDR` (ip:port), `DATABASE_GUARD_TLS_CA` (its CA
    /// file) and `DATABASE_GUARD_TLS_PASSWORD` (user `postgres`); skipped
    /// without them.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_relayed_connection_verifies_the_certificate_for_the_host_name() {
        use sqlx::Connection;
        let (Ok(address), Ok(ca), Ok(password)) = (
            std::env::var("DATABASE_GUARD_TLS_ADDR"),
            std::env::var("DATABASE_GUARD_TLS_CA"),
            std::env::var("DATABASE_GUARD_TLS_PASSWORD"),
        ) else {
            eprintln!("skipped: DATABASE_GUARD_TLS_ADDR, _CA or _PASSWORD is not set");
            return;
        };
        let address: SocketAddr = address.parse().unwrap();
        let directory = relay::to(address, "tls-test").await.unwrap();
        let connect = |host: &'static str| {
            let url = format!(
                "postgres://postgres:{password}@{host}:{}/postgres?sslmode=verify-full&sslrootcert={ca}",
                address.port()
            );
            let relayed = through_relay(&url, host, &directory).unwrap();
            async move { sqlx::postgres::PgConnection::connect(&relayed).await }
        };
        let mut connection = connect("db.test")
            .await
            .expect("db.test is the certificate's name");
        let one: i32 = sqlx::query_scalar("SELECT 1")
            .fetch_one(&mut connection)
            .await
            .unwrap();
        assert_eq!(one, 1);
        let Err(refused) = connect("wrong.test").await else {
            panic!("wrong.test is not the certificate's name");
        };
        assert!(matches!(refused, sqlx::Error::Tls(_)), "{refused}");
    }

    /// A tenant brings a relay to at most RELAYS_PER_TENANT addresses, so it
    /// cannot use up the process's; another tenant still gets its own.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_tenant_cannot_use_up_the_relays() {
        let mut listeners = Vec::new();
        for _ in 0..=relay::RELAYS_PER_TENANT {
            listeners.push(tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap());
        }
        let addresses: Vec<SocketAddr> =
            listeners.iter().map(|l| l.local_addr().unwrap()).collect();
        for address in &addresses[..relay::RELAYS_PER_TENANT] {
            relay::to(*address, "greedy").await.unwrap();
        }
        // Again to one it already has: fine.
        relay::to(addresses[0], "greedy").await.unwrap();
        let refused = relay::to(addresses[relay::RELAYS_PER_TENANT], "greedy")
            .await
            .unwrap_err();
        assert!(refused.contains("this tenant"), "{refused}");
        relay::to(addresses[relay::RELAYS_PER_TENANT], "other")
            .await
            .unwrap();
    }

    /// The relay dials the address it was started for, whatever the name
    /// would resolve to now: a byte sent to the relay reaches that address.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_relay_dials_only_its_vetted_address() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let vetted = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let directory = relay::to(vetted.local_addr().unwrap(), "t").await.unwrap();
        let socket = format!(
            "{directory}/.s.PGSQL.{}",
            vetted.local_addr().unwrap().port()
        );
        let mut client = tokio::net::UnixStream::connect(&socket).await.unwrap();
        client.write_all(b"ping").await.unwrap();
        let (mut accepted, _) = vetted.accept().await.unwrap();
        let mut received = [0u8; 4];
        accepted.read_exact(&mut received).await.unwrap();
        assert_eq!(&received, b"ping");
    }

    #[tokio::test]
    async fn an_allowlisted_host_is_admitted_as_is() {
        let url = "postgres://u:p@localhost:5432/db";
        assert_eq!(
            admit_with(url, "t", |host| host == "localhost")
                .await
                .unwrap(),
            url
        );
    }

    #[test]
    fn pinning_replaces_only_the_host() {
        assert_eq!(
            pin(
                "postgres://u:p%40x@db.example.com:6543/app?sslmode=require",
                "203.0.113.7".parse().unwrap()
            )
            .unwrap(),
            "postgres://u:p%40x@203.0.113.7:6543/app?sslmode=require"
        );
        assert_eq!(
            pin(
                "postgres://u@db.example.com/app",
                "2001:db8::1".parse().unwrap()
            )
            .unwrap(),
            "postgres://u@[2001:db8::1]/app"
        );
    }
}
