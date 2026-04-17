# runtara-connections

Connection and credential management for Runtara — CRUD, OAuth2, at-rest encryption, and rate limiting for SaaS integrations.

## What it is

This crate owns the storage and serving of connection/credential configs used by Runtara integrations. It persists connection metadata and encrypted parameter blobs in Postgres, handles OAuth2 authorization flows, resolves provider auth (including AWS SigV4 signing) for runtime HTTP calls, and tracks rate-limit state in Redis/Valkey.

Two surfaces are exposed: an Axum router bundle (`connections_router`, `admin_router`, `oauth_callback_router`, `runtime_router`) for HTTP mounting, and a `ConnectionsFacade` for in-process use from host code. State is carried by `ConnectionsConfig` / `ConnectionsState`; at-rest encryption goes through the `CredentialCipher` trait (`cipher_from_env` is the default). Multi-tenant isolation is enforced at the repository layer via `TenantId` and explicit `tenant_id` arguments on every facade call.

## Using it standalone

Not a standalone crate — `publish = false`, and it assumes the host supplies a `PgPool`, an optional Redis URL, an encryption key, and an Axum app to mount routers on. The intended pattern is:

```rust
let config = runtara_connections::ConnectionsConfig {
    db_pool, redis_url, public_base_url, http_client,
    cipher: runtara_connections::cipher_from_env(),
};
let facade = Arc::new(runtara_connections::ConnectionsFacade::new(
    runtara_connections::ConnectionsState::from_config(config.clone()),
));
app.merge(runtara_connections::connections_router(config.clone()))
   .merge(runtara_connections::oauth_callback_router(config));
```

The host adds auth middleware and chooses the mount prefix.

## Inside Runtara

- Consumed exclusively by `runtara-server`, which mounts all four routers (`connections_router`, `admin_router`, `runtime_router`, `oauth_callback_router`) and holds an `Arc<ConnectionsFacade>` in `AppState`.
- The facade is pulled into server subsystems for runtime credential resolution: `channels/session.rs`, `api/services/webhook_verification.rs`, and `api/services/agent_testing.rs`.
- Depends on `runtara-dsl` for connection-type metadata (integration ids, auth types, parameter schemas).
- Uses `sqlx` against Postgres for storage, `redis` for rate-limit counters, and `aes-gcm` + `zeroize` for the envelope-encrypted parameter blobs defined in `crypto`.
- Runs in-process inside the management-plane server binary (native tokio), not in WASM guests — agents reach secrets only via the internal `runtime_router` that `runtara-server` proxies.

## License

AGPL-3.0-or-later.
