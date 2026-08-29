# Install & update

Runtara ships as a self-contained bundle: one server binary, the prebuilt WASM
agent components, and the licence set. Nothing is compiled on your machine, and
no Rust toolchain is installed — workflow compilation happens in-process.

For running Runtara from Docker or from a source checkout instead, see the
[README](../README.md).

## Requirements

| | |
|---|---|
| OS / arch | Linux or macOS, on `x86_64` or `aarch64` |
| Database | PostgreSQL, three databases (platform, object model, server) |
| Cache | Valkey or Redis |
| Tools | `curl`, `tar` |

The object-model and server databases need the `pg_trgm`, `vector`, and
`fuzzystrmatch` extensions. The server does **not** create them for you:

```bash
psql -d runtara_server -c 'CREATE EXTENSION IF NOT EXISTS pg_trgm; CREATE EXTENSION IF NOT EXISTS vector; CREATE EXTENSION IF NOT EXISTS fuzzystrmatch;'
```

Run the same three statements against the object-model database.

## Install

```bash
curl -fsSL https://install.runtara.com | sh
```

This resolves the latest release, downloads the installer from it, then fetches
and verifies the matching bundle. Run as root for a system install; run as an
ordinary user for a user install (see [Layout](#layout)).

Flags go after `sh -s --`:

```bash
curl -fsSL https://install.runtara.com | sh -s -- --user --version 1.6.10
```

| Flag | Effect |
|---|---|
| `--user` / `--system` | Force the install mode instead of auto-detecting from your uid |
| `--version <v>` | Pin a version (`--version dev` installs the rolling dev build) |
| `--skip-service` | Install the bundle but do not register or start a service |
| `--run` | Implies `--skip-service`, then runs the server in the foreground |
| `--uninstall` | Remove the bundle and service |
| `--purge` | With `--uninstall`, also delete config and data |
| `--docker` | Generate and start a Docker Compose stack instead of installing natively |
| `--docker-persist` | As `--docker`, with named volumes so data survives `down` |

### Docker mode

`--docker` writes a `docker-compose.yml`, a Dockerfile, and a DB init script to
`~/.runtara-docker` (override with `--docker-dir`) and brings up the server plus
PostgreSQL and Valkey. Without `--docker-persist` the volumes are ephemeral and
data is lost on `docker compose down`. `SERVER_PORT` and `TENANT_ID` from your
environment are baked into the generated file.

The repository's own root `docker-compose.yml` is the better starting point if
you want to edit the stack — the generated one is meant to be disposable.

### Offline / local bundle

If you already have a release tarball, skip the download:

```bash
./install.sh --bundle /path/to/runtara-1.6.10-aarch64-linux.tar.gz
./install.sh --bundle-dir /path/to/extracted-bundle
```

## Layout

The installer picks its paths from the install mode.

| | System (root) | User (Linux) | User (macOS) |
|---|---|---|---|
| Bundle | `/opt/runtara` | `~/.runtara` | `~/.runtara` |
| Config | `/etc/runtara` | `~/.config/runtara` | `~/Library/Application Support/runtara` |
| Data | `/var/lib/runtara` | `~/.local/share/runtara` | `~/Library/Application Support/runtara/data` |
| Logs | `/var/log/runtara` | (journal) | `~/Library/Logs/runtara` |
| On `PATH` | `/usr/local/bin/runtara-server` | — | — |

A system install also creates a `runtara` service user (`_runtara` on macOS) and
gives it ownership of the data and log directories.

Inside the bundle: `bin/runtara-server`, `agents/` (each agent as a `.wasm` plus
a sibling `.meta.json`, alongside the shared workflow components), `licenses/`,
a `VERSION` file, and `MANIFEST.json` recording the version, commit, rustc
version, component counts, target, and build date.

## Configuration

The installer writes `<config-dir>/runtara-server.conf` — a plain
`KEY=value` environment file, mode `640`. **An existing config is never
overwritten**, so upgrades keep your settings; new options have to be added by
hand.

Values are seeded from your environment at install time, so this works:

```bash
export RUNTARA_SERVER_DATABASE_URL=postgres://runtara:secret@db/runtara_server
export OBJECT_MODEL_DATABASE_URL=postgres://runtara:secret@db/runtara_objects
export RUNTARA_DATABASE_URL=postgres://runtara:secret@db/runtara
export VALKEY_HOST=cache.internal
export AUTH_PROVIDER=oidc OAUTH2_ISSUER=https://id.example.com/
curl -fsSL https://install.runtara.com | sh
```

Export them — a `VAR=x curl … | sh` prefix binds to `curl`, not to the shell
that runs the installer.

Unset database URLs fall back to `postgres://runtara:password@localhost/…`,
which is a placeholder, not a working default — edit the file before starting
the server in anything but a throwaway setup.

Keys the installer manages: `TENANT_ID`, `SERVER_HOST`, `SERVER_PORT` (7001),
the three database URLs, `VALKEY_HOST` / `VALKEY_PORT` / `VALKEY_PASSWORD`, the
`RUNTARA_MCP_*` session settings, `AUTH_PROVIDER` and its mode-specific vars,
`RUNTARA_AGENT_COMPONENTS_DIR` (points at the bundle's `agents/`), `DATA_DIR`,
and `RUST_LOG`.

### Authentication

`AUTH_PROVIDER` is `oidc`, `trust_proxy`, or `local`; the installer rejects
anything else. It defaults to `oidc` and binds `0.0.0.0`. For the other two
modes it defaults `SERVER_HOST` to `127.0.0.1` — **the server refuses to start
in those modes on a non-loopback address**, because authentication is happening
somewhere other than in-process. See
[deployment/auth-modes.md](deployment/auth-modes.md).

## Service management

Unless you passed `--skip-service`, the installer registers and starts a
service: a systemd unit on Linux (`/etc/systemd/system/runtara-server.service`,
or `~/.config/systemd/user/` for a user install) or a launchd job on macOS
(`/Library/LaunchDaemons/` or `~/Library/LaunchAgents/com.runtara.server.plist`).

```bash
systemctl restart runtara-server        # add --user for a user install
journalctl -fu runtara-server
```

```bash
launchctl list | grep runtara           # macOS
tail -f ~/Library/Logs/runtara/runtara-server.log
```

## Update

Re-run the installer. There is no `self-update` subcommand.

```bash
curl -fsSL https://install.runtara.com | sh
```

It compares the release version against the installed `VERSION` file and exits
without touching anything if they match. Otherwise it stages the new bundle
beside the old one, stops the service, swaps the directories, and restarts —
so an interrupted download leaves the running install untouched. Your config
and data directories are not modified.

Pin a version with `--version`, including downgrades.

## Uninstall

```bash
curl -fsSL https://install.runtara.com | sh -s -- --uninstall
```

Stops and deregisters the service and deletes the bundle, keeping your config
and data. Add `--purge` to delete those too. Databases are never touched.

## Troubleshooting

| Exit | Meaning |
|---|---|
| 10 / 11 | Unsupported OS or CPU architecture |
| 40 | Could not resolve the version, download the bundle, or find a `VERSION` file inside it |
| 41 | Checksum verification failed — treat as a corrupt or tampered download |
| 1 | Invalid `AUTH_PROVIDER` |

If no `.sha256` is published next to the tarball the installer warns and
continues without verifying.

A user-mode systemd service stops when your session ends unless lingering is
enabled (`loginctl enable-linger $USER`).
