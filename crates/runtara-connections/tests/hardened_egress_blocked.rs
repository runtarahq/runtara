//! Without the dev allowlist env, the hardened client's guarded resolver must
//! refuse hosts that resolve to private/internal addresses (own test binary:
//! the allowlist is read once per process and must stay EMPTY here).

#[tokio::test]
async fn hardened_client_blocks_private_hosts_by_default() {
    // Ensure fail-closed: no allowlist in this process.
    unsafe { std::env::remove_var("RUNTARA_PROXY_ALLOWED_HOSTS") };

    let client = runtara_connections::net::build_hardened_client();
    let err = client
        .post("http://localhost:1/token")
        .send()
        .await
        .expect_err("loopback egress must be rejected");
    let msg = format!("{err:#}");
    let mut source = std::error::Error::source(&err);
    let mut chain = msg.clone();
    while let Some(s) = source {
        chain.push_str(&format!(" | {s}"));
        source = s.source();
    }
    assert!(
        chain.contains("private/internal"),
        "expected the guarded-resolver rejection in the error chain, got: {chain}"
    );
}
