# Suggested Improvements

Design doc capturing the next DX fixes requested: wiring the OCI runner via env-driven configuration, and honoring connection timeouts in the QUIC client/SDKs. No code changes yet.

## Context
- Management start/stop currently always returns “Runner not configured” because `ManagementHandlerState` is started without a runner.
- Connect timeout knobs exist in both SDK configs but are ignored by `RuntaraClient`, so users cannot tune handshake timeouts.

## Goals
- Enable the intended OCI runner behind an environment switch, without forcing it in all deployments.
- Make connection timeouts functional and consistent across SDKs.

## Proposal: Runner Integration (OCI via env)
- Decision: Opt-in via `RUNTARA_RUNNER=oci`. When set, `main` constructs `OciRunner::from_env()` and calls `ManagementHandlerState::with_runner(...)` before starting the management QUIC server. When unset, start/stop stays disabled but logs a clear warning during startup.
- Env surface for OCI runner (existing code in `OciRunnerConfig::from_env`):
  - `RUNTARA_RUNNER=oci` (new gate)
  - `DATA_DIR` (default `.data`, made absolute) – run I/O + logs
  - `BUNDLES_DIR` (default `${DATA_DIR}/bundles`) – OCI bundles
  - `EXECUTION_TIMEOUT_SECS` (default 300)
  - `USE_SYSTEMD_CGROUP` (default false)
- Behavior changes:
  - Startup logs: whether the runner was attached; warn once if not configured so operators know start/stop will be rejected.
  - Management handler keeps current validation but now works when the env gate is set.
- Docs to add:
  - Root README/ops doc with a “Enable OCI runner” section describing the envs and expected crun/OCI prerequisites.
  - Clarify that management traffic is QUIC, not HTTP, even on the “admin” port.

## Proposal: Implement Connect Timeouts
- Add `connect_timeout_ms` to `RuntaraClientConfig` (protocol crate) and propagate it from both SDK configs (`runtara-sdk` and `runtara-management-sdk`).
- In `RuntaraClient::connect`, wrap the `endpoint.connect(...).await` in `tokio::time::timeout(Duration::from_millis(connect_timeout_ms), …)`. If it elapses, return a new `ClientError::Timeout`.
- Map timeout into SDK errors:
  - `runtara-sdk`: new `SdkError::Connection(ClientError::Timeout(..))` path with a clear message.
  - `runtara-management-sdk`: same mapping via existing `From<ClientError>`.
- Defaults stay the same (10s), but the value now actually governs handshake time.
- Tests: unit test for default propagation, and an integration-ish test that forces a tiny timeout to ensure the error surfaces (feature-gated if needed to avoid flaky CI).

## Rollout Steps (sequential, separate PRs)
1) Document: add root README/ops notes for OCI runner enablement and admin-port protocol clarification.
2) Code: gate runner with `RUNTARA_RUNNER=oci`, wire `OciRunner::from_env()` into `main`, add startup log/warn.
3) Code: implement connect timeout plumbing + error surface; add small tests.
4) Verify: run examples with runner enabled and with runner disabled to ensure behavior is explicit; run SDK timeout tests. 
