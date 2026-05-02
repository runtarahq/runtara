---
name: tail-logs
description: Use to set up the right RUST_LOG filters and tail server + environment logs scoped to a specific tenant, instance, or component. Saves the "what's the right log filter again" lookup. Pair with trace-instance when you need both API state and live log output.
---

# Tail logs scoped to what you're debugging

Server logs are the source of truth for the runtime / WASM runner / DB layer. Default `RUST_LOG=info` is usually too noisy and `debug` is too much; this skill picks the right slice.

## Filter recipes

The server respects per-crate `RUST_LOG` directives. Common slices:

### Default (e2e-verify / general dev)

```bash
RUST_LOG="runtara_server=info,runtara_environment=info,runtara_core=info"
```

### Capability execution (debugging an agent)

```bash
RUST_LOG="runtara_environment=debug,runtara_agents=debug,runtara_workflows=info,runtara_server=warn"
```

`runtara_agents=debug` shows the per-capability inputs/outputs. `runtara_environment=debug` shows the WASM runner lifecycle.

### Connection / OAuth flow

```bash
RUST_LOG="runtara_connections=debug,runtara_server=info,hyper=warn"
```

Use when token refresh or OAuth callback isn't behaving. Mute `hyper` or it floods.

### DSL compile / step registration

```bash
RUST_LOG="runtara_dsl=debug,runtara_compile=debug,runtara_workflows=info"
```

Useful when a step type isn't being picked up or schema generation is off.

### Migrations / startup issues

```bash
RUST_LOG="sqlx=info,runtara_server=debug"
```

`sqlx=info` shows applied migrations; bump to `debug` to see every query (very noisy).

## Apply

If the server is already running, restart it with the new `RUST_LOG` — it's read once at startup, not per-request:

```bash
# kill the previous instance
pkill -x runtara-server

# relaunch with the focused filter (full env block from e2e-verify)
RUST_LOG="runtara_environment=debug,runtara_agents=debug,runtara_workflows=info,runtara_server=warn" \
DATABASE_URL="postgres://user:pass@localhost:5432/runtara_e2e_server" \
OBJECT_MODEL_DATABASE_URL="postgres://user:pass@localhost:5432/runtara_e2e_server" \
RUNTARA_DATABASE_URL="postgres://user:pass@localhost:5432/runtara_e2e_test" \
RUNTARA_ENVIRONMENT_ADDR="127.0.0.1:18002" \
DATA_DIR="/tmp/runtara_e2e_data" \
SERVER_PORT=17001 \
INTERNAL_PORT=17002 \
  target/debug/runtara-server 2>&1 | tee /tmp/runtara-server.log
```

## Tail and grep

If you `tee`'d to a file (recommended), tail with grep:

```bash
# scope to a specific instance
tail -f /tmp/runtara-server.log | grep --line-buffered "<INSTANCE_ID>"

# scope to a specific tenant
tail -f /tmp/runtara-server.log | grep --line-buffered 'tenant_id=<TENANT>'

# show only WARN+ across crates
tail -f /tmp/runtara-server.log | grep --line-buffered -E ' (WARN|ERROR) '
```

## Quick rules of thumb

- Match log noise to the hypothesis: default to `info`, drop to `warn` for crates you trust, bump to `debug` only for the suspect crate.
- Don't ship a `RUST_LOG=trace,*` everywhere — it'll hide the signal in volume.
- If you're already tailing logs **and** stepping through API responses (`trace-instance`), the logs almost always tell you the cause first; check there before adding more API calls.
