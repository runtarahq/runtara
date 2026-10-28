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
pub(crate) async fn admit(database_url: &str) -> Result<String, String> {
    if !crate::config::database_connection_guard() {
        return Ok(database_url.to_owned());
    }
    // The platform's own database is the operator's, not a tenant's.
    if crate::config::object_model_database_url().as_deref() == Some(database_url) {
        return Ok(database_url.to_owned());
    }
    admit_with(database_url, host_is_allowlisted_for_egress).await
}

async fn admit_with(
    database_url: &str,
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
        let directory = relay::to(SocketAddr::new(address, options.get_port())).await?;
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

    /// The most addresses one process relays to.
    pub(super) const MAX_RELAYS: usize = 256;

    /// The relay directory for each vetted address.
    fn relays() -> &'static Mutex<HashMap<SocketAddr, String>> {
        static RELAYS: OnceLock<Mutex<HashMap<SocketAddr, String>>> = OnceLock::new();
        RELAYS.get_or_init(Mutex::default)
    }

    /// The directory of the relay to `address`, started on first use. sqlx
    /// connects to `{directory}/.s.PGSQL.{port}`.
    #[cfg(unix)]
    pub(super) async fn to(address: SocketAddr) -> Result<String, String> {
        use std::os::unix::fs::DirBuilderExt;

        let mut relays = relays().lock().expect("database relays poisoned");
        if let Some(directory) = relays.get(&address) {
            return Ok(directory.clone());
        }
        if relays.len() >= MAX_RELAYS {
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
        tokio::spawn(serve(listener, address));
        let directory = directory
            .to_str()
            .ok_or("the database relay's directory is not UTF-8")?
            .to_owned();
        relays.insert(address, directory.clone());
        Ok(directory)
    }

    /// No Unix sockets: a host name cannot be both verified and pinned.
    #[cfg(not(unix))]
    pub(super) async fn to(_: SocketAddr) -> Result<String, String> {
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
            assert!(admit_with(url, nothing_allowlisted).await.is_err(), "{url}");
        }
    }

    #[tokio::test]
    async fn a_public_address_is_admitted_and_pinned() {
        let admitted = admit_with(
            "postgres://u:p@8.8.8.8:5433/db?sslmode=require",
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
        let admitted = admit_with(verified, nothing_allowlisted).await.unwrap();
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
            admit_with(verified, nothing_allowlisted).await.unwrap(),
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
        let directory = relay::to(address).await.unwrap();
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

    /// The relay dials the address it was started for, whatever the name
    /// would resolve to now: a byte sent to the relay reaches that address.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_relay_dials_only_its_vetted_address() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let vetted = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let directory = relay::to(vetted.local_addr().unwrap()).await.unwrap();
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
            admit_with(url, |host| host == "localhost").await.unwrap(),
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
