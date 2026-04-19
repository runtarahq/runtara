# Runtara Installation & Update — Design Plan

Status: **Draft**
Owner: Platform
Supersedes: previous `.deb` packaging (removed)

## Goals

A single, script-based installation and update path for Runtara that:

1. Works on **Ubuntu**, **Debian**, **Amazon Linux**, **RHEL/Fedora**, and **macOS on Apple Silicon**.
2. Is **bootstrappable via `curl`** in one line, no prior tooling required on the host.
3. **Supports idempotent updates** — re-running the installer or calling a `self-update` subcommand upgrades an existing install safely and atomically.
4. **Uses Runtara GitHub Releases** as the single source of truth for the latest version.
5. **Starts on boot** — systemd on Linux, launchd on macOS.
6. **Does not require the user to install or manage a Rust toolchain.** The compiler Runtara needs is an implementation detail of Runtara, not a dependency the customer maintains.
7. **Does not touch the user's existing tooling.** Any pre-existing `rustc`, `cargo`, or `wasmtime` on the host is ignored. The bundled runtime is isolated under a single directory.

## Non-goals

- Windows support (tracked separately).
- Package-manager–native packaging (`.deb`, `.rpm`, Homebrew formula). A Homebrew tap may be added later as an *additive* channel; not a prerequisite.
- Orchestration of PostgreSQL / Valkey. Users still bring their own data stores.

## Current state (what we're replacing)

- [scripts/install.sh](../scripts/install.sh) — Linux-only. Installs rustup, WASI SDK, clones runtara source, compiles `runtara-workflow-stdlib` from scratch on the host (multi-minute build), then installs a systemd unit. Runtara's `rustc` dependency is exposed to the user as "here, let us install rustup into `/usr/local/rustup`" — a surprising intrusion for someone who just wanted a workflow engine.
- [.github/workflows/release.yml](../.github/workflows/release.yml) — previously built one `x86_64-linux` tarball + `.deb` packages. Now replaced with the bundle matrix build.

### Problems

1. The installer exposes Runtara's internal dependency on `rustc` to the user. Rustup is added to the host, `/etc/profile.d/rust.sh` is written, `$CARGO_HOME` and `$RUSTUP_HOME` are created. If the user already had a Rust toolchain, it's now competing with ours.
2. Fresh install is multi-minute because the stdlib is compiled from source on every host.
3. No aarch64 Linux or macOS support.
4. ~~`.deb` packaging duplicated the install logic~~ (now removed).
5. No update mechanism — users manually re-run the installer with no guarantee of atomicity.
6. The Linux binary is dynamically linked against the GitHub runner's glibc (currently 2.39 on `ubuntu-latest`). Hosts with older glibc see `GLIBC_2.39 not found` at startup.

## The core design decision

Runtara invokes `rustc` on every workflow compile ([crates/runtara-workflows/src/compile.rs:551](../crates/runtara-workflows/src/compile.rs#L551)). Workflow compilation is foundational to the product — it happens whenever a user saves or publishes a workflow via the no-code UI, on the hot path, in every deployment.

There are two ways to deliver that compiler to the host:

- **Expose it as a dependency** — install rustup, pin rustc, manage versions, coordinate CI artifacts. (What the old plan did.)
- **Bundle it as an implementation detail** — ship a hermetic directory with the exact rustc Runtara was built and tested against, invoke it via absolute path, never touch the user's environment.

**We bundle.** The compiler is part of Runtara's runtime, not a prerequisite the customer installs. This is the model used by JetBrains IDEs, Android Studio, GitLab Omnibus, Unity, Unreal Engine, and every major low-code platform (OutSystems, Mendix, LabVIEW). Shipping a self-contained bundle is the paved road for products in this category.

## Design: the hermetic bundle

Every release produces one **all-in-one tarball per (os, arch)** containing Runtara plus every runtime dependency it needs to compile and execute workflows. Installing Runtara means extracting one tarball into one directory and wiring up a service unit. That's the entire story.

### Bundle layout

```
/opt/runtara/                          # single directory, fully self-contained
  bin/
    runtara-server                     # the whole thing — API, compilation, execution
    wasmtime                           # bundled Wasmtime CLI (invoked as a subprocess)
  toolchain/                           # hermetic Rust toolchain (NEVER on PATH)
    bin/
      rustc
      cargo                            # only if needed; may be removable
    lib/
      rustlib/
        {host-target}/                 # native libs for compiling proc-macros
        wasm32-wasip2/                 # target libs for workflow WASM
  stdlib/                              # pre-built Runtara workflow stdlib
    libruntara_workflow_stdlib.rlib
    deps/
      *.rlib                           # wasm32-wasip2 transitive deps
      *.so | *.dylib                   # host proc-macros, built against bundled rustc
  licenses/                            # redistribution compliance (see §Licensing)
    LICENSE-runtara-AGPL-3.0
    LICENSE-rust-APACHE-2.0
    LICENSE-rust-MIT
    LICENSE-wasmtime-APACHE-2.0
    NOTICE-rust
    THIRD-PARTY-NOTICES.md
  VERSION                              # single version stamp for the whole bundle
  MANIFEST.json                        # { runtara, rustc, wasmtime } versions + checksums
```

**Nothing in `toolchain/` is ever added to `$PATH`.** `runtara-server` sets `RUSTC=/opt/runtara/toolchain/bin/rustc` when spawning the workflow compile subprocess. The user's shell, user's Rust install (if any), and system package manager are all untouched.

**Single version stamp.** The bundled rustc, the bundled stdlib rlibs, the proc-macros, and the server binary are all produced by the same CI job against the same pinned rustc. They ship together. No separate manifests to verify — if you have the bundle, the parts are consistent by construction.

**Wasmtime is bundled.** One less thing for users to install, one less version to match, consistent behavior across hosts.

### Artifacts per release

| Artifact | Target triple | CI runner |
|---|---|---|
| `runtara-{ver}-x86_64-linux.tar.gz` | `x86_64-unknown-linux-gnu` | `ubuntu-22.04` (glibc 2.35) |
| `runtara-{ver}-aarch64-linux.tar.gz` | `aarch64-unknown-linux-gnu` | `ubuntu-22.04-arm` or `cross` on `ubuntu-22.04` |
| `runtara-{ver}-aarch64-darwin.tar.gz` | `aarch64-apple-darwin` | `macos-14` (Apple Silicon) |
| `*.sha256` sibling for every tarball | — | — |
| `install.sh` | — | hand-written, attached to release |
| `SHA256SUMS` | — | aggregated, optionally signed |

**No Intel macOS.** `x86_64-apple-darwin` is not a supported target. Apple shipped the last Intel Mac in 2023; Apple Silicon is the only target worth building for a product launching now. Users on Intel Macs who want to run Runtara locally can use a Linux VM.

Tarball size estimate: 300–500 MB per target. Comparable to a rustup install, which users are already accustomed to for similar tooling.

**Linux libc strategy:** build against glibc on `ubuntu-22.04` (glibc 2.35). Covers Ubuntu 22.04+, Debian 12+, Amazon Linux 2023, RHEL 9+. Amazon Linux 2 and older are explicitly not supported. We deliberately avoid musl — the workflow-native-compile path that previously motivated it has been replaced by `wasm32-wasip2`, and the host binary has no cross-distro requirements beyond "modern Linux". Cleanup of the dead musl code in [crates/runtara-workflows/src/compile.rs](../crates/runtara-workflows/src/compile.rs), [.cargo/config.toml](../.cargo/config.toml), and [Cargo.toml:47-48](../Cargo.toml#L47-L48) is tracked as a follow-up.

**No `.deb` packages.** The `.deb` path and `packaging/` directory have been removed.

## How the bundle is built in CI

Per-target job on the matching runner:

1. **Acquire upstream Rust.** Download the official unmodified Rust distribution tarballs from `https://static.rust-lang.org/dist/` for the pinned version and target:
   - `rust-{ver}-{host-target}.tar.gz` — rustc, cargo, host std
   - `rust-std-{ver}-wasm32-wasip2.tar.gz` — wasm target libs
   Extract and prune to just the components Runtara needs (drop docs, `rust-src`, other targets, dev tools we don't invoke).
2. **Build `runtara-server`** using the bundled rustc. This ensures the server and the compiler it invokes at runtime are built from the exact same toolchain.
3. **Build the workflow stdlib** twice against the bundled rustc:
   - `cargo build -p runtara-workflow-stdlib --release --target wasm32-wasip2 --no-default-features` → target rlibs
   - `cargo build -p runtara-workflow-stdlib --release` → host proc-macro `.so`/`.dylib`
4. **Fetch Wasmtime** from upstream for this target, extract the CLI binary.
5. **Assemble the bundle directory** per the layout above, including `licenses/`, `VERSION`, and `MANIFEST.json`.
6. **Tar + sha256.** Upload as workflow artifact.

Final `release` job:

1. Collect all matrix artifacts.
2. Generate `SHA256SUMS` (optionally sign with minisign/cosign).
3. `gh release create "$TAG" ...` with all bundles, checksums, and `install.sh`.

The existing `publish-*` crates.io jobs stay as-is — those are about publishing the Runtara source crates to crates.io for library consumers, unrelated to the binary bundle.

### Rust version pinning

One `rust-toolchain.toml` at the repo root pins the rustc version used by every CI build (library builds and bundle builds alike). Bumping Rust means one PR: bump the toolchain file, merge, cut a release. Every bundled host picks up the new version on its next update.

No `manifest.json` verification on the host, no cross-tarball skew checks, no version drift. The pinning is entirely a CI-side concern.

## Bootstrap (the one-liner)

```sh
curl -fsSL https://github.com/runtarahq/runtara/releases/latest/download/install.sh | sh
```

### What the installer actually does

Radically simpler than the old plan. One tarball, one directory, one service unit.

```
1. Detect OS + arch from uname
2. Detect system vs user mode (root or --user flag)
3. Resolve target version (latest from GitHub API, or --version flag)
4. Detect existing install via /opt/runtara/VERSION (or user-mode equivalent)
     - same version → noop, exit 0
     - downgrade → refuse unless --force
5. Download runtara-{ver}-{target}.tar.gz + .sha256
6. Verify checksum (fail hard on mismatch)
7. Extract to /opt/runtara.new
8. Stop existing service (if upgrading)
9. Atomic swap: mv /opt/runtara /opt/runtara.old && mv /opt/runtara.new /opt/runtara
10. Write config file (first install only; preserved on update unless --force-config)
11. Create service user (system mode only, first install)
12. Install service unit (systemd or launchd)
13. Start service
14. Clean up /opt/runtara.old on successful start; leave it for rollback on failure
15. Print summary
```

That's the whole script. No rustup phase, no WASI SDK phase, no stdlib compile phase, no source clone phase, no proc-macro juggling, no toolchain pinning negotiation.

### Non-default invocations

```sh
# Pin a specific version by substituting `latest` with the tag:
curl -fsSL https://github.com/runtarahq/runtara/releases/download/v1.5.0/install.sh | sh

# Flags are passed after `sh -s --`:
curl -fsSL https://github.com/runtarahq/runtara/releases/latest/download/install.sh | sh -s -- --user
curl -fsSL https://github.com/runtarahq/runtara/releases/latest/download/install.sh | sh -s -- --uninstall
curl -fsSL https://github.com/runtarahq/runtara/releases/latest/download/install.sh | sh -s -- --uninstall --purge
curl -fsSL https://github.com/runtarahq/runtara/releases/latest/download/install.sh | sh -s -- --skip-service
```

Non-interactive mode works the same as today — export env vars (`RUNTARA_NONINTERACTIVE=1`, `RUNTARA_DATABASE_URL`, etc.) and the installer skips prompts.

### One binary, one installer — no `--component` flag

Today, Runtara ships as a single `runtara-server` binary. The `runtara-environment` crate exists but is linked into `runtara-server` as a library dependency ([runtara-server/Cargo.toml:27](../crates/runtara-server/Cargo.toml#L27)) — there is no separate execution-host deployable to install. One bundle, one binary, one service unit. The installer has no component selection because there's only one thing to install.

A future split of compilation or execution into a dedicated sidecar (e.g. `runtara-compiler`) would be the moment to introduce a `--component` flag. Until then, the installer stays flag-free on this axis. The bundle's `toolchain/` + `stdlib/` are always present because `runtara-server` always needs them (to compile workflows).

## Install layout on the host

| Path | Linux (system) | Linux (user) | macOS (system) | macOS (user) |
|---|---|---|---|---|
| Bundle | `/opt/runtara` | `~/.runtara` | `/opt/runtara` | `~/.runtara` |
| Symlinks into `/usr/local/bin` | yes | n/a | yes | n/a |
| Config | `/etc/runtara/runtara-server.conf` | `~/.config/runtara/runtara-server.conf` | `/etc/runtara/runtara-server.conf` | `~/Library/Application Support/runtara/runtara-server.conf` |
| Data | `/var/lib/runtara` | `~/.local/share/runtara` | `/var/lib/runtara` | `~/Library/Application Support/runtara/data` |
| Logs | `journalctl -u runtara-server` | `journalctl --user -u runtara-server` | `/var/log/runtara/` | `~/Library/Logs/runtara/` |
| Service user | `runtara:runtara` (created) | current user | `_runtara` (created) | current user |
| Service unit | `/etc/systemd/system/runtara-server.service` | `~/.config/systemd/user/runtara-server.service` | `/Library/LaunchDaemons/com.runtara.server.plist` | `~/Library/LaunchAgents/com.runtara.server.plist` |

**Config and data live outside the bundle.** `/opt/runtara` is owned by the installer and wiped on every update. Config lives at `/etc/runtara/`, data at `/var/lib/runtara/`. This keeps updates atomic without losing state.

**Symlinks into `/usr/local/bin`.** The installer creates `/usr/local/bin/runtara-server` → `/opt/runtara/bin/runtara-server` so users can run `runtara-server --version` without knowing about `/opt/runtara`. Wasmtime is deliberately *not* symlinked — it's Runtara's private wasmtime, not a system tool.

## Service management

A small shell abstraction in `install.sh` picks the right backend at runtime:

```sh
service_install   runtara-server
service_enable    runtara-server
service_start     runtara-server
service_restart   runtara-server
service_stop      runtara-server
service_remove    runtara-server
```

**Linux — systemd unit:**

```ini
[Unit]
Description=Runtara Server
After=network-online.target
Wants=network-online.target
Documentation=https://runtara.com/docs

[Service]
Type=simple
User=runtara
Group=runtara
EnvironmentFile=/etc/runtara/runtara-server.conf
ExecStart=/opt/runtara/bin/runtara-server
Restart=on-failure
RestartSec=5

# Hardening
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
ReadWritePaths=/var/lib/runtara /var/log/runtara

[Install]
WantedBy=multi-user.target
```

For user-mode installs, the same unit is written to `~/.config/systemd/user/` and loaded via `systemctl --user enable --now runtara-server`. Optionally `loginctl enable-linger $USER` so it survives logout (documented as dev-only).

**macOS — launchd plist (`/Library/LaunchDaemons/com.runtara.server.plist`):**

```xml
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
    <key>Label</key>                  <string>com.runtara.server</string>
    <key>ProgramArguments</key>       <array>
        <string>/opt/runtara/bin/runtara-server</string>
    </array>
    <key>RunAtLoad</key>              <true/>
    <key>KeepAlive</key>              <true/>
    <key>UserName</key>               <string>_runtara</string>
    <key>GroupName</key>              <string>_runtara</string>
    <key>EnvironmentVariables</key>   <dict>
        <key>RUNTARA_CONFIG</key>     <string>/etc/runtara/runtara-server.conf</string>
    </dict>
    <key>StandardOutPath</key>        <string>/var/log/runtara/runtara-server.log</string>
    <key>StandardErrorPath</key>      <string>/var/log/runtara/runtara-server.err</string>
    <key>ProcessType</key>            <string>Background</string>
</dict>
</plist>
```

Loaded with `launchctl bootstrap system /Library/LaunchDaemons/com.runtara.server.plist` (system) or `launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.runtara.server.plist` (user).

## Update paths

Two complementary mechanisms, both built on atomic directory swap.

### 1. Re-run the installer

```sh
curl -fsSL https://github.com/runtarahq/runtara/releases/latest/download/install.sh | sh
```

Idempotent. Detects existing install, skips if already current, otherwise runs the full bootstrap against the current directory. Atomic because the swap happens at the `mv` step — either the new bundle is in place, or the old one still is.

### 2. Built-in `self-update` subcommand

```sh
runtara-server self-update              # apply latest
runtara-server self-update --check      # exit 0 if up-to-date, 1 if update available
runtara-server self-update --version=X.Y.Z
```

Flow:

1. `GET https://api.github.com/repos/runtarahq/runtara/releases/latest`
2. Compare `tag_name` with `env!("CARGO_PKG_VERSION")`.
3. If newer: download the matching bundle for `$(uname)` + `$(arch)` and its `.sha256`.
4. Verify checksum.
5. Extract to `/opt/runtara.new`.
6. `systemctl stop runtara-server` (or launchctl equivalent).
7. Atomic swap: `mv /opt/runtara /opt/runtara.old && mv /opt/runtara.new /opt/runtara`.
8. `systemctl start runtara-server`.
9. On successful restart → `rm -rf /opt/runtara.old`. On failure → swap back, report error, leave service stopped.

**Zero version drift.** Because the whole toolchain + stdlib + binary ship in one tarball, a partial update is impossible. Either everything is v1.5.0 or everything is v1.4.9 — never mixed.

**Rollback:** if `/opt/runtara.old` still exists after a failed update, rolling back is `mv /opt/runtara /opt/runtara.failed && mv /opt/runtara.old /opt/runtara && systemctl restart`.

**Background update checks (optional, post-MVP):** the server logs "update available: vX.Y.Z" on a daily timer. No automatic application without explicit user action.

## Uninstall

```sh
curl -fsSL https://github.com/runtarahq/runtara/releases/latest/download/install.sh | sh -s -- --uninstall
```

- Stops and removes the service unit.
- Removes `/opt/runtara` (or `~/.runtara`).
- Removes `/usr/local/bin/runtara-server` symlink.
- Leaves config and data in place by default. `--purge` removes `/etc/runtara` and `/var/lib/runtara` after a confirmation prompt.
- Removes the service user only if it was created by a previous install and nothing else owns files under its home.

## Licensing compliance

Runtara is AGPL-3.0-or-later ([Cargo.toml:33](../Cargo.toml#L33), [LICENSING.md](../LICENSING.md)), with a commercial license available from SyncMyOrders. The bundled components and their licenses:

| Component | License | Redistributable? |
| --- | --- | --- |
| `rustc`, `cargo`, Rust stdlib | Apache-2.0 OR MIT | Yes |
| `wasm32-wasip2` target libs | Apache-2.0 OR MIT | Yes |
| `wasmtime` | Apache-2.0 WITH LLVM-exception | Yes |
| `runtara-*` | AGPL-3.0-or-later | (our own) |

**No GPL/copyleft components in the bundle.** No license-compatibility problems.

### Key points

1. **Aggregation, not derivative work.** Shipping rustc alongside Runtara in a tarball is aggregation under both the Apache/MIT and AGPL terms. The AGPL applies to Runtara's code, not to rustc, even though they live in the same directory.
2. **Subprocess invocation is not linking.** `Command::new("rustc")` at [compile.rs:551](../crates/runtara-workflows/src/compile.rs#L551) is a clean process boundary. The FSF has been explicit for decades that subprocess invocation does not create a combined work.
3. **Redistribution obligations are trivial.** Apache-2.0 and MIT require including the license text and preserving copyright notices. The `licenses/` directory inside the bundle satisfies both:

    ```text
    /opt/runtara/licenses/
      LICENSE-runtara-AGPL-3.0
      LICENSE-rust-APACHE-2.0
      LICENSE-rust-MIT
      LICENSE-wasmtime-APACHE-2.0
      NOTICE-rust
      THIRD-PARTY-NOTICES.md
    ```

    `THIRD-PARTY-NOTICES.md` contains an index of each third-party component, its license, and a link to upstream source (`rust-lang/rust` at the pinned tag, `bytecodealliance/wasmtime` at the pinned tag). Apache/MIT do not require us to *ship* upstream source — pointing at it discharges the obligation.
4. **AGPL §13 network disclosure.** Applies to modifications of Runtara served over a network. Unaffected by bundling rustc. Users running stock Runtara have nothing additional to disclose.

### Trademark compliance

"Rust" and the Rust logo are trademarks of the [Rust Foundation](https://foundation.rust-lang.org/policies/logo-policy-and-media-guide/). Their trademark policy permits:

- Redistributing unmodified upstream Rust binaries.
- Saying "powered by Rust" or similar.

It does **not** permit:

- Shipping a modified `rustc` under the name "Rust" or "rustc".
- Branding a product as "Runtara Rust" or similar.

**CI rule (hard):** the bundle job pulls unmodified upstream Rust from `https://static.rust-lang.org/dist/` and does not patch it. Pruning unused components (docs, `rust-src`, unused targets) is not modification in a trademark sense — we're selectively copying the upstream distribution, not changing contents. If we ever need to patch rustc, we rename and document it.

### Pre-existing question — worth flagging separately

When Runtara compiles a user workflow, the output links against `runtara-workflow-stdlib`, which is AGPL. The compiled WASM artifact is arguably a derivative work of AGPL code.

- For users running workflows **inside their own Runtara instance**: fine, they're using the product, not distributing or modifying anything.
- For users **extracting the compiled WASM and running it elsewhere**: the AGPL obligations arguably travel with the artifact. This may already be intentional (part of the commercial-license pitch) but should be documented explicitly in [LICENSING.md](../LICENSING.md) if so. It's a product-level question, not an install-plan question.

## Prior art — why this shape is well-trodden

The hermetic-bundle pattern is the standard for products that embed a toolchain:

- **JetBrains IDEs** (IntelliJ, PyCharm, GoLand, RustRover, Android Studio) — every IDE ships its own JBR (JetBrains Runtime, a JDK fork) plus language compilers. Pinned per IDE version, updated atomically, doesn't touch the user's system Java. Closest structural match to what we're building.
- **GitLab Omnibus** — `curl | bash` install to `/opt/gitlab`, bundles Ruby, PostgreSQL, Redis, nginx, Prometheus, Grafana. Hundreds of MB. Atomic updates via directory swap. The canonical "bundle everything, install in one shot" reference.
- **OutSystems / Mendix / LabVIEW** — low-code/no-code platforms that bundle language runtimes and compile user-authored logic via the bundled toolchain. Direct product-category analogues.
- **Unity / Unreal Engine** — ship Mono/IL2CPP/C++ toolchain pieces used to compile user scripts.
- **Electron apps** (Slack, VS Code, Discord, Figma, Postman, 1Password) — each ships its own Chromium + Node. Users don't blink.
- **Docker Desktop** — ships its own Linux VM, kernel, and qemu.
- **Anaconda / Miniconda, Julia, Deno, Bun** — single-tarball runtimes, drop anywhere and run.

The unusual pattern would be the *opposite*: a no-code platform whose install instructions begin with "first, install rustup". None of the above products do that, for exactly the reasons we're moving away from it.

## CI changes

Rewrite [.github/workflows/release.yml](../.github/workflows/release.yml) to produce one bundle per (os, arch). Matrix:

```yaml
strategy:
  matrix:
    include:
      - { target: x86_64-unknown-linux-gnu,   runner: ubuntu-22.04,     os: linux,  arch: x86_64  }
      - { target: aarch64-unknown-linux-gnu,  runner: ubuntu-22.04-arm, os: linux,  arch: aarch64 }
      - { target: aarch64-apple-darwin,       runner: macos-14,         os: darwin, arch: aarch64 }
```

Per-target job:

1. Download upstream Rust dist tarballs for the target from `static.rust-lang.org`.
2. Extract to a staging toolchain directory; prune unused components.
3. `cargo build --release -p runtara-server` using the staging toolchain.
4. Build stdlib rlibs (`--target wasm32-wasip2`) and host proc-macros using the staging toolchain.
5. Fetch Wasmtime for this target from upstream; extract CLI.
6. Assemble the bundle directory layout including `licenses/` (committed in the repo at [docs/licenses/](licenses/) and copied in).
7. Tar + sha256. Upload artifact.

Final release job aggregates artifacts, generates `SHA256SUMS`, and creates the GitHub release. Crucially, it also uploads [scripts/install.sh](../scripts/install.sh) as a release asset — this is what the `curl | sh` one-liner points at via `/releases/latest/download/install.sh`. If `install.sh` isn't in the release's asset list, the bootstrap URL 404s.

The crate-publish jobs (`publish-dsl`, `publish-core`, etc.) are unchanged — those publish source to crates.io for library consumers and are independent of the binary bundle.

## Hosting the bootstrap script

We serve `install.sh` directly from the GitHub release assets. No Cloudflare Worker, no branded domain, no extra infrastructure. The release workflow uploads [scripts/install.sh](../scripts/install.sh) as a release asset alongside the bundle tarballs, and GitHub's stable `/releases/latest/download/install.sh` alias handles the "latest" resolution.

**Why not `raw.githubusercontent.com`?** That URL is tied to branch state (`main` HEAD), which means churn on `main` would reach users immediately. Release assets are tied to tags, so the user always gets the script that matches a released version.

**Version pinning** is done by substituting `latest` with an explicit tag in the URL:

```sh
curl -fsSL https://github.com/runtarahq/runtara/releases/download/v1.5.0/install.sh | sh
```

**A branded `install.runtara.com` is a future option**, not a prerequisite. If branding, query-param version pinning (`?version=1.5.0`), or browser-UA routing ever becomes useful, adding a thin Cloudflare Worker in front of the same GitHub URL is a small follow-up that doesn't break existing users.

**Script-as-release-asset means one change to the release workflow:** the `gh release create` step must include `scripts/install.sh` in its asset list, so every release has the matching script attached.

## Security

- **Checksums:** every tarball has a `.sha256` sibling; the installer verifies before extracting. Hard-fail on mismatch.
- **Signatures (post-MVP):** sign `SHA256SUMS` with `cosign` (Sigstore, keyless via OIDC) or `minisign` (simple, offline). Leaning `minisign` for operational simplicity. The installer can optionally verify if a public key is embedded. Start without signatures, add as a follow-up.
- **TLS only** for all downloads.
- **No piping to root without review.** Users wanting to audit the script first can fetch it from GitHub directly.
- **macOS Gatekeeper:** MVP uses ad-hoc signing. Notarization is tracked as a follow-up. Server installs via `curl | sh` do not set the `com.apple.quarantine` xattr, so Gatekeeper does not intervene.

## Rollout

1. **Phase 1 — licenses directory + CI bundle build.** Add `docs/licenses/` with all third-party license texts and `THIRD-PARTY-NOTICES.md`. Prototype the bundle-build CI job on one target (x86_64-linux) to validate the approach end-to-end: download upstream Rust, build Runtara against it, build stdlib + proc-macros, assemble bundle, install locally, compile a trivial workflow. No release changes yet.
2. **Phase 2 — full matrix.** Extend to aarch64-linux and aarch64-darwin.
3. **Phase 3 — installer rewrite.** Rewrite [scripts/install.sh](../scripts/install.sh) around the bundle: detect platform, download bundle, verify, atomic swap, install service. Add `--user`, `--uninstall`, `--version`, `--purge` flags.
4. **Phase 4 — self-update subcommand.** Add `runtara-server self-update` in Rust. Uses the same atomic-swap flow as the installer.
5. **Phase 5 — ~~delete `.deb` packaging~~.** Done. `packaging/`, `scripts/build-deb.sh`, and the old `install.sh` have been removed.
6. **Phase 6 — docs.** Rewrite the installation section of user docs around the one-liner. (No domain or worker setup — we serve `install.sh` from GitHub release assets directly.)
7. **Follow-up — dead code cleanup.** Remove the vestigial workflow-native-compile code in [compile.rs](../crates/runtara-workflows/src/compile.rs) (musl target, `get_host_target`, `+crt-static`), and the `musl`-related bits of [.cargo/config.toml](../.cargo/config.toml) and [Cargo.toml:47-48](../Cargo.toml#L47-L48) now that workflows compile to WASM exclusively.

## Open questions

1. **Rustc build vs download.** Should the CI job download upstream Rust tarballs from `static.rust-lang.org`, or build upstream Rust from source against the pinned tag? Downloading is faster and bit-for-bit matches upstream (trademark-safe). Building from source gives us supply-chain provenance. Recommendation: **download**. Building rustc from source is a multi-hour job per target and provides no meaningful benefit unless we need to patch it.
2. **Wasmtime version pinning.** Same question — download upstream releases, or build from source? Recommendation: **download**. Wasmtime publishes official binaries for all our targets.
3. **Service user on macOS.** Create a dedicated `_runtara` user via `dscl` (proper convention) or run as root? Recommendation: **create `_runtara`**. Tracked as a Phase 3 decision.
4. **Signature scheme.** `cosign` (Sigstore, keyless via OIDC) vs `minisign` (simple, offline-verifiable). Recommendation: **minisign** for operational simplicity; revisit if supply-chain provenance becomes a requirement.
5. **User-mode on Linux.** `systemctl --user` + linger is clean but uncommon in production. Recommendation: **support it but document as dev-only.**
6. **Homebrew tap.** Additive channel for macOS dev users. Low effort once tarballs exist. Recommendation: **track as a follow-up, not a prerequisite.**
7. **Bundle size target.** Rough estimate is 300–500MB per bundle. Worth measuring once the first prototype is built and setting a hard cap (e.g. "alert if any bundle exceeds 600MB") in CI.
8. **When to introduce a `--component` split.** Not now. Revisit when either (a) there is a concrete scaling requirement that a single `runtara-server` binary can't meet, or (b) the compiler is factored into a separate `runtara-compiler` binary. At that point, add a `--component` flag with the new values; don't retrofit it speculatively.

## Success criteria

- One-liner install works on: Ubuntu 22.04+, Ubuntu 24.04, Debian 12+, Amazon Linux 2023, RHEL 9+, and macOS 14+ (Apple Silicon only).
- Fresh install completes in well under a minute on a 1Gbps connection, bottlenecked only by the tarball download. No multi-minute host-side compile.
- Update from version N to N+1 via `self-update` completes in under 30 seconds, dominated by download time. Atomic: either fully new or fully old, never mixed.
- Re-running the installer is a noop if already at the target version.
- Uninstall leaves no stray files beyond config + data (unless `--purge`).
- `runtara-server` never touches `$PATH`, `$CARGO_HOME`, `$RUSTUP_HOME`, or any user-owned rustc install.
- CI end-to-end test on every supported platform: install → compile a trivial workflow → execute → uninstall. Gates releases.
- Bundle licenses directory validated in CI: every third-party component in `toolchain/`, `stdlib/`, and `bin/wasmtime` has a corresponding license entry in `licenses/` and an entry in `THIRD-PARTY-NOTICES.md`.
