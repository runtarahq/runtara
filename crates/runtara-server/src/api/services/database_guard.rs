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
//! public address for the check and a private one for the connect. A
//! `sslmode=verify-full` URL keeps its host name, which certificate
//! verification needs, and is checked but not pinned.

use std::net::IpAddr;
use std::str::FromStr;

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
    // Certificate verification needs the host name: checked, not pinned.
    if matches!(
        options.get_ssl_mode(),
        sqlx::postgres::PgSslMode::VerifyFull
    ) {
        return Ok(database_url.to_owned());
    }
    let address = addresses[0];
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
        // Host-name verification keeps the host name.
        let verified = "postgres://u:p@8.8.8.8/db?sslmode=verify-full";
        assert_eq!(
            admit_with(verified, nothing_allowlisted).await.unwrap(),
            verified
        );
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
