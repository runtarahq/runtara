# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> This file tracks release notes starting at **1.7.0**. Earlier tagged releases
> (`v1.0.21` through `v1.6.18`) are available as git tags; their history lives
> in `git log`.

## [1.8.0] - 2026-04-13

### Added

- Compilation queue for serialized scenario compilation.

### Changed

- Internal API default port moved from `7001` to `7002`. `7001` remains the
  public `runtara-server` HTTP API port; `7002` is the internal service port.

### Fixed

- Agent testing dispatcher routing.
- `Default` impl for `ExecutionGraph` so downstream tests compile.
- sqlx offline cache miss in CI by switching `sqlx::query!` → `sqlx::query_as`.

## [1.7.0] - 2026-04-10

### Added

- Automatic rate-limit honoring for integrations: 429 responses (and equivalent
  provider codes) trigger durable sleep until the indicated `retry_after`
  without consuming the normal retry budget. Configurable via
  `AUTO_RETRY_ON_429`, `MAX_429_RETRIES`, and `MAX_RETRY_DELAY_MS`.

## Earlier releases

Tagged releases `v1.0.21` (2026-04-15) through `v1.6.18` predate this
changelog. Notable platform-level changes during that period — reconstructed
from the workspace crates and configuration — include:

- **New crates:** `runtara-server` (HTTP API server embedding environment +
  core), `runtara-connections` (connection/credential management),
  `runtara-object-store` (schema-driven dynamic PostgreSQL object model),
  `runtara-http` (portable HTTP client for native/WASI/browser-wasm),
  `runtara-ai` (WASM-first LLM completion client), `runtara-text-parser`
  (Slack/SMS/CLI text-channel adapter).
- **Scenario compilation targets WASM by default.** The default
  `RUNTARA_COMPILE_TARGET` is `wasm32-wasip2`; the native-musl path is
  retained as a fallback and flagged for cleanup.
- **Valkey/Redis is now a required runtime dependency** for
  `runtara-server` scenario execution (`VALKEY_HOST` env var).
- **Wasm is the default runner** in `runtara-environment`; OCI, Native, and
  Mock runners remain available.
- **Removed:** `runtara-protocol` crate (never existed on main; the reference
  in earlier documentation was stale).
