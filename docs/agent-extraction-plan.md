# Agent Extraction Plan — OSS / Proprietary Split

**Status:** Draft for review
**Date:** 2026-07-14
**Goal:** Support a cloud deployment model where the OSS distribution is `runtara core + OSS agents` and the commercial distribution is `runtara core + OSS agents + proprietary/custom agents`, by moving agents out of the monorepo (fully or partially) and adding a proprietary-agent overlay to cloud provisioning.

---

## 1. The good news: discovery already works this way

Agent discovery is **entirely directory-driven and is the only supported mode**. There is no static baked-in agent list at runtime.

- `RUNTARA_AGENT_COMPONENTS_DIR` points at a directory. At boot, `ComponentDispatcherService::from_dir` (`crates/runtara-component-host/src/dispatcher.rs:94`) walks it and loads every `runtara_agent_*.wasm` + sibling `runtara_agent_*.meta.json` pair. That parsed set becomes the production `AgentCatalog` — the static `runtara_agents::static_registry` is only a test/fallback shim.
- The server **hard-fails** if the dir is unset or loads zero agents (`crates/runtara-server/src/server.rs:936`). The runtime dispatches *only* components physically present in that dir.
- The same directory also feeds compile-time composition: `direct_wasm_components_dir` defaults to `agent_components_dir` (`crates/runtara-server/src/config.rs:445`) and the direct compiler's `wac compose` pulls agent components from there.

**Implication:** "mount/copy agents into a dir and it discovers them" is literally and exactly the model today. No core runtime change is needed for discovery. The whole project is about (a) splitting the *build*, (b) formalizing the *SDK contract* agents depend on, (c) rewiring *CI* to produce agent artifacts across repos, and (d) adding a provisioning *overlay*.

---

## 2. Grounding facts (from codebase research)

### 2.1 The agent → core coupling surface is small and clean
Every one of the 26 agents (`crates/agents/runtara-agent-*`) depends on exactly these shared crates (path deps today):

| Crate | Used by | Provides | Publishability |
|---|---|---|---|
| `runtara-agent-wit` (WIT package `runtara:agent@0.3.0`) | all 26 | the WIT contract + WASI 0.2.3 deps tree + `templates/agent.wit.in` | clean (WIT only) |
| `runtara-agent-macro` | all 26 | `#[capability]`, `#[capability_input/output]`, `#[connection_params]` proc macros | clean (syn/quote/darling only) |
| `runtara-dsl` (`default-features=false`) | all 26 | `agent_meta::{AgentInfo, CapabilityMeta, …}` + `coercion::coerce_input` | **entangled** — it's also the central server/workflow DSL crate |
| `runtara-http` | integration agents (38×) | HTTP client (native `ureq` / wasm `wasi:http`) | clean |
| `runtara-ai` | ai-tools, bedrock, openai | LLM provider abstraction | clean |
| `runtara-agent-encoding` | csv, xml, text | shared text decode | clean |

**No agent depends on `runtara-agents`, `runtara-workflows`, `runtara-server`, `runtara-connections`, or the workflow components.** Agents are cleanly decoupled from the server and engine. Verified by grep — zero matches.

### 2.2 The WIT contract is execution-only with zero custom host imports
- `world agent { export capabilities; }` where `capabilities.invoke(capability-id, input: list<u8>, connection: option<connection-info>) -> result<list<u8>, error-info>` — opaque JSON-in-bytes.
- The agent world declares **no** custom host imports. Its only imports are WASI 0.2.3 (`wasi:http`, `wasi:cli/environment`, clocks, random, io, sockets, filesystem), satisfied by the host linker.
- Per-agent `wit/agent.wit` is **auto-generated** by each crate's `build.rs` from `crates/runtara-agent-wit/templates/agent.wit.in` — not hand-maintained.

### 2.3 The load-bearing runtime contract is invisible to WIT ⚠️
Agents reach internal services over `wasi:http` to URLs read from **env vars**, not host imports:
- `RUNTARA_HTTP_PROXY_URL` (credential injection for HTTP integrations), `RUNTARA_AGENT_SERVICE_URL` (native-forward + object-model), `RUNTARA_OBJECT_MODEL_URL`, `RUNTARA_TENANT_ID`.
- Headers: `X-Org-Id`, `X-Runtara-Connection-Id`.

This env/header convention is **as load-bearing as the WIT but not captured by it** — a WIT version bump won't catch drift here. It must be codified and versioned explicitly.

### 2.4 The macro hard-wires `runtara_dsl::` — the key coupling
`runtara-agent-macro` expands to `runtara_dsl::agent_meta::*` and injects `runtara_dsl::coercion::coerce_input(...)` **inside the wasm binary** (`crates/runtara-agent-macro/src/lib.rs:382,419`). An external agent crate **must** have a crate importable as `runtara_dsl`. This is the single hardest coupling to sever cleanly.

### 2.5 Three agents are native-forward and cannot leave core
`sftp`, `xlsx`, `compression` do no real work in-component — their `.wasm` shell forwards a JSON envelope to `POST /api/internal/agents/{module}/{capability}` and the host runs native code (`ssh2`/libssh2, `calamine`, `zip`) that **cannot compile to wasm32-wasip2**. The host half lives in `crates/runtara-agents` (`static_registry.rs` — a hardcoded compile-time array; there is **no dynamic native-plugin loader**). These three are inseparable from the trusted core.

The other 23 agents are pure-WASM: 7 pure-compute (crypto, csv, datetime, text, transform, utils, xml) + 15 proxy-backed + object-model. All extract cleanly.

### 2.6 The credential boundary constrains proprietary agents
WASM never sees secrets — it carries an opaque `connection_id`; the host resolves parameters and injects Authorization/base-URL/AWS-signing (proxy path: `internal_proxy.rs`; native path: `internal_agents.rs`). Consequences:
- A **pure-WASM proprietary agent that reuses an existing connection type** (api-key, generic OAuth, AWS) is a safe, drop-in untrusted `.wasm` blob.
- A proprietary agent needing a **new connection/auth type** still requires a core change: a connection-params struct + `HttpConnectionExtractor` in the connection subsystem. The cred boundary stays in core.
- A proprietary **native-forward** agent would need its native half compiled into the trusted core — unsupported today (no plugin loader).

### 2.7 Build → bundle → ship pipeline (today, monorepo)
1. `scripts/build-agent-components.sh`: discovers agents by grepping workspace `Cargo.toml` members; `cargo component build --release --target wasm32-wasip2 -p <agent>` for each; also builds the 2 shared workflow components (stdlib + runtime); then `cargo run -p runtara-agent-bundle-emit --bin emit-meta` writes the `.meta.json` sidecars. Gate: `wasm_count == meta_count`.
2. `runtara-agent-bundle-emit` is a **host binary** that statically links all agent crates, calls each crate's host-only `agent_info()`, and serializes `meta.json`. It hardcodes the agent list twice (`main.rs` vec + `Cargo.toml` path deps).
3. `scripts/build-bundle.sh` assembles `bin/runtara-server` + `agents/` (all wasm+meta **and** the 2 workflow components) + licenses + `MANIFEST.json` → tarball.
4. Docker image is built **FROM the linux bundle tarball** (not from source), sets `ENV RUNTARA_AGENT_COMPONENTS_DIR=/opt/runtara/agents`, published multi-arch to `ghcr.io/runtarahq/runtara`.
5. Toolchain is tightly pinned everywhere: `cargo-component 0.21.1`, WASI `0.2.3`, `wasm-tools 1.252.0`, `wasmtime 43.0.0`, target `wasm32-wasip2`. Drift traps the composed `workflow.wasm` at runtime.

### 2.8 Provisioning (smo-provisioning) delivers agents inside the bundle
- No Terraform/Ansible/containers for the runtime — GitHub Actions + composite actions SSH into pre-existing Ubuntu VMs; runtara runs as a **systemd service** installed from the release bundle via the official installer (`curl install.sh | sudo bash --version <X>`), which extracts to `/opt/runtara/` including `/opt/runtara/agents/*.wasm`+`.meta.json`.
- `deploy-runtara.sh` ensures `RUNTARA_AGENT_COMPONENTS_DIR=/opt/runtara/agents` and chmods the agent `.wasm` to 644. **The installer overwrites `/opt/runtara/agents` fresh on every deploy** — any overlay must be re-applied each deploy.
- Per-tenant version pinning via `tenants/*.yml` `runtara-version:`. Config merged (defaults × tenant) into `/etc/runtara/runtara-server.conf`, restart-if-changed.
- **An entitlements allow-list already exists** (commented, `tenants/syncmyorders.yml`): `RUNTARA_ENTITLEMENTS_JSON` with `"agents": ["http", …]`, enforced *inside* runtara-server at runtime. This gates *which agents a tenant may run*, independent of *which `.wasm` are on disk* — the natural per-tenant lever.

---

## 3. Two topologies (the pivotal decision)

Both satisfy the business goal (OSS vs commercial). They differ enormously in effort and in how "plugin-like" OSS agents become.

### Topology A — Minimal: only proprietary agents get a new repo
OSS agents **stay in the runtara monorepo** (they're already AGPL/OSS and already ship in the bundle). Only proprietary agents move to a new private repo that git-pins the SDK contract from the public runtara repo. Provisioning overlays the proprietary `.wasm` on entitled tenants.

- **Effort:** low. No OSS SDK extraction, no OSS agent migration, no byte-identical re-verification. Core essentially unchanged.
- **Delivers:** exactly the OSS-vs-commercial split you described.
- **Trade-off:** OSS agents remain built-in (not independently versioned/releasable). The "everything is a plugin" story is only half-realized.

### Topology B — Full extraction: all OSS agents → separate public repo
Move the 23 pure agents to a public `runtara-agents` repo; proprietary agents to a private repo; runtara core keeps only the 3 native-forward agents + the 2 workflow components. All three repos consume the shared SDK contract.

- **Effort:** high. Requires SDK extraction/versioning, WIT vendoring across repos, cross-repo CI, and a byte-identical/behaviorally-identical migration of 23 agents.
- **Delivers:** clean plugin architecture; OSS agents independently versioned; smallest core.
- **Trade-off:** ongoing 3-repo WIT/ABI coordination overhead.

> **Recommendation:** Ship **Topology A first** — it meets the commercial goal with ~30% of the work, and *every* piece of A (SDK git-pin path, provisioning overlay, entitlements wiring) is also a prerequisite for B. Treat B as the north star you grow into once the proprietary path is proven. The phased plan below is written so A is Phases 0/3/4 and B adds Phases 1/2.

---

## 4. Key decisions

1. **Topology A vs B** (see §3). **DECIDED: Topology A for v1** — proprietary-only split; OSS agents stay in the monorepo/bundle. B (full extraction) remains the staged north star but is deferred; the registry/marketplace (§9) is the more important north star.
2. **SDK distribution mechanism.** **DECIDED for v1: git-tag pin** — proprietary agents' `Cargo.toml` points `runtara-dsl`/`runtara-agent-macro`/etc. at `{ git = "…runtara", tag = "sdk-vX" }`; zero macro change; WIT vendored via submodule. The registry/marketplace (§9) later forces a **published lean `runtara-agent-sdk`** for third-party Rust authors — you cannot ask external authors to git-pin internal crates. So: git-pin now, carve-and-publish when third-party Rust authoring opens.
3. **Native-forward proprietary agents:** out of scope for v1 (no host plugin loader), and **permanently excluded from the marketplace** (§9) — a native-forward agent needs trusted host code and can't be a distributable untrusted blob. Proprietary/marketplace v1 = pure-WASM + proxy agents that reuse existing connection types.
4. **Bundle contents:** the OSS Docker image ships OSS agents (so `docker run` works out of the box); the commercial image = OSS image + proprietary overlay `COPY`'d at build time.

---

## 5. Phased plan

### Phase 0 — Freeze the contract (runtara repo, no behavior change) — required for A and B
- Define the "agent SDK" crate set (§2.1) as the versioned contract; introduce an `sdk-vX.Y.Z` git tag scheme aligned to the WIT `runtara:agent@` version.
- Write `docs/agent-abi-contract.md` codifying the **runtime env/header contract** (§2.3) — the invisible-to-WIT part. Add a contract-conformance test.
- Confirm the `meta.json` schema (owned by `runtara-dsl::agent_meta`) is part of the SDK version surface.
- (Optional, defer) carve `runtara-agent-sdk` out of `runtara-dsl`.

### Phase 1 — Stand up `runtara-agents` OSS repo (Topology B only)
- New public repo `runtarahq/runtara-agents` (AGPL to match).
- Move the **23 pure agents**. Keep `sftp`/`xlsx`/`compression` **entirely in core** (shell + native half — inseparable, and OSS anyway). Core's `build-agent-components.sh` then builds only those 3 + the 2 workflow components.
- Consume SDK via git-tag pin. **WIT caveat:** cargo-component resolves WIT target deps from a filesystem path or a component registry — *not* git. So the agents repo needs the `runtara-agent-wit/wit/` tree on disk via **git submodule** (or a vendoring sync step) with a `wasm-tools component wit` drift check. (Formal alternative: publish the WIT package to a warg/wa.dev registry.)
- Port `build-agent-components.sh` + a slimmed `emit-meta` (still depends on the SDK for `agent_info()`/`AgentInfo`).
- CI: build 23 → wasm+meta; run per-agent `--lib` tests; publish `runtara-agents-oss-<version>-<arch>.tar.gz` (or an OCI artifact) to a GitHub release / GHCR.
- **Acceptance:** the 23 agents' wasm+meta are byte-identical (or behaviorally identical via `runtara-component-host` integration tests) to the monorepo build.

### Phase 2 — Rewire runtara core to consume the OSS artifact (Topology B only)
- runtara release CI: download the pinned OSS-agents artifact + build the 3 native-forward + 2 workflow components in-tree, then merge into `bundle/agents/`.
- Update `build-bundle.sh assemble_bundle` and the `wasm_count == meta_count` gate to `expected = in-tree(3) + artifact(N)`.
- Core pins an OSS-agents version in `MANIFEST.json`; decide lockstep vs independent bumps.
- Delete `crates/agents/*` for the 23 migrated agents; verify bundle + smoke + `direct_wasm_execute` e2e green.

### Phase 3 — Proprietary agents repo (Topology A and B) — the core commercial deliverable
- New private repo `runtara-agents-pro`, same SDK contract (same `sdk-vX` git-tag + same WIT submodule), same build → `runtara-agents-pro-<version>-<arch>.tar.gz`.
- **Constraints (call out to authors):** pure-WASM only; reuse existing connection types for drop-in; a *new* connection/auth type still needs a core change (extractor + proxy) — that's the one place proprietary agents touch core.
- Build a **commercial Docker image**: `FROM ghcr.io/runtarahq/runtara:<ver>` + `COPY` proprietary agents into `/opt/runtara/agents` (private GHCR).

### Phase 4 — Provisioning overlay (smo-provisioning) — required for the systemd (non-container) fleet
- `scripts/deploy-runtara.sh`: **after** the installer extracts the bundle and **alongside** the existing 644 chmod, add an **idempotent overlay step** — if the tenant is entitled, fetch `runtara-agents-pro-<ver>.tar.gz` from private GHCR/release (new token secret) and extract wasm+meta into `/opt/runtara/agents`, then re-chmod 644. Must run every deploy (installer wipes the dir). Version-match to `RUNTARA_VERSION` to avoid WIT/ABI drift.
- `.github/workflows/deploy-runtara.yml`: add `RUNTARA_PROPRIETARY_TOKEN` secret; pass a per-tenant proprietary flag/version into the deploy step.
- `tenants/*.yml` + `config.yml`: add a `proprietary_agents_version:` selector (parsed in `parse-tenant-config`), and enable the existing `RUNTARA_ENTITLEMENTS_JSON.agents` allow-list so runtara-server *also* gates usage at runtime.
- Result: **two independent levers** — what's on disk (overlay) and what a tenant may run (entitlements JSON).
- (Optional) `setup-smo-runtime-instance.sh`: add `RUNTARA_AGENT_COMPONENTS_DIR` to the initial conf write; `update-runtara-version.sh`: bump proprietary version in lockstep.

---

## 6. Cross-cutting risks & mitigations

| Risk | Mitigation |
|---|---|
| **WIT/ABI drift** across repos → composed `workflow.wasm` traps | Single source of truth = `runtara-agent-wit` @ version; all repos pin the same WIT tag + `cargo-component 0.21.1` + WASI `0.2.3`; `wasm-tools component wit` drift check in each repo's CI; a shared `toolchain.env`. |
| **Env/header runtime contract invisible to WIT** (§2.3) | Codify in `docs/agent-abi-contract.md`, version it, add a conformance test. |
| **`meta.json` schema is a cross-repo contract** | Ship it as part of the SDK version; changes require an SDK minor bump. |
| **`bindings.rs` churn** (regenerated every `cargo component build`) | CI discipline in each repo: revert-unless-WIT-changed. |
| **Native-forward proprietary agents** | Unsupported v1; would require a host native-plugin (dlopen-style) loader — future work. |
| **New connection types for proprietary integrations** | Still a core change (connection-params struct + `HttpConnectionExtractor` + proxy). Reusing existing connection types = drop-in. Document the boundary. |
| **Overlay wiped on every deploy** | Re-apply in `deploy-runtara.sh` every run (it runs on every deploy); never rely on persistence in `/opt/runtara/agents`. |
| **emit-meta hardcodes the agent list** | Whichever repo owns emit-meta must keep its list in sync with its workspace members; consider deriving from members instead of a literal vec. |

---

## 7. Safe migration sequencing
1. **Phase 0** contract freeze (+ optional SDK carve) — runtara, no behavior change.
2. **Phase 3 first if choosing Topology A:** stand up `runtara-agents-pro` with one pilot proprietary agent; git-pin SDK; build the commercial image.
3. **Phase 4:** wire the provisioning overlay + entitlements on a **dev tenant** (e.g. `syncmyorders`/`agilevision`, already on `dev`); verify end-to-end; roll out.
4. **(Topology B, later)** Phase 1 → prove parity of the 23 agents → Phase 2 flip core CI to consume the artifact and delete in-tree agents → re-verify bundle/smoke/e2e.

---

## 8. Bottom line
Discovery is done and is the only supported path, and the agent→core coupling is small (4 shared crates, zero custom WIT host imports). The real work is contract-hardening + cross-repo CI + a provisioning overlay — **not** the discovery mechanism. Topology A delivers the OSS/commercial split quickly and is a strict subset of the full extraction (Topology B), so starting with A wastes no effort toward B. The two hard edges to respect throughout: the `runtara_dsl` macro coupling (§2.4) and the fact that native-forward agents and new connection types still live in the trusted core (§2.5–2.6).

---

## 9. North star: `agents.runtara.com` — registry + marketplace

The chosen direction is a registry **and** marketplace at `agents.runtara.com` for **WASM-only** agents, where an agent is either (a) a first-party/proprietary hand-written component, or (b) **a workflow (incl. AI steps) compiled and published as an agent**. Topology A (§3) is the foundation; the registry is the north star that the provisioning overlay generalizes into.

### 9.1 Why WASM-only is a hard admission rule, not a preference
The credential boundary (§2.6) is what makes a third-party marketplace safe: an untrusted marketplace component only ever sees an opaque `connection_id`; the host injects real secrets at the proxy. A **native-forward** agent needs trusted host code compiled into the core (§2.5) — it therefore **cannot** be a distributable marketplace blob. WASM-only is the security invariant that makes untrusted publishing viable. Enforce it at publish time.

### 9.2 The registry is close to a solved-shape problem
The runtime already consumes exactly what a registry serves: `.wasm` + `.meta.json` pairs in a directory (§1). So the instance-side path is "pull the tenant's entitled agents → stage into `/opt/runtara/agents` → auto-discovered." WASM components are OCI artifacts and the project already publishes to GHCR, so:
- **Backing store:** an OCI registry (agents as OCI artifacts).
- **`agents.runtara.com`:** OCI registry + an index/search/metadata API + a web UI for browse/install.
- **Provisioning:** the Phase-4 overlay (§5) generalizes from "fetch one proprietary tarball" to "reconcile the tenant's entitled agent set from the registry," still re-applied every deploy, still gated by `RUNTARA_ENTITLEMENTS_JSON.agents`.

### 9.3 Design in from day one (painful to retrofit)
- **Signing / provenance.** Publisher identity + signed components (cosign/sigstore over the OCI artifacts); the runtime **verifies signatures before loading**. This is what lets a tenant trust an agent's origin even though it's sandboxed.
- **ABI version as a first-class field.** Each agent declares the `runtara:agent@X` WIT version + WASI version it was built against; `meta.json` carries an `abiVersion`; a runtara vN **refuses incompatible agents**. The registry enforces compatibility (§6 drift risk at ecosystem scale).
- **Per-tenant entitlement/install flow.** Installing an agent = add it to the tenant's components set + the runtime allow-list. The two levers already exist (on-disk overlay + entitlements JSON, §2.8).

### 9.4 Workflow-as-agent (the accessibility play) — new machinery + constraints
Compiling a workflow and publishing it as an agent reuses direct-WASM composition + the embedded workflow runner + `EmbedWorkflow`, but:
- **Gap:** a compiled workflow exports the *workflow-runtime* world, not the *agent* world (`capabilities.invoke`). Needs a new compile target / wrapper component that exports `invoke`, with the capability I/O schema derived from the workflow's I/O schema and `meta.json` generated from the workflow definition (not Rust macros).
- **Constraint 1 — synchronous only.** `invoke` is single-shot; suspending workflows (waits/signals/human-in-the-loop) don't fit. v1 = non-suspending workflows, or add a resumable agent interface.
- **Constraint 2 — collapsed durability.** From the parent's view the workflow-agent is one atomic leaf step; a retry re-runs the whole child. Acceptable if ~idempotent; document it.
- **Constraint 3 — declared connection + LLM needs.** A workflow-agent consumes the *installing tenant's* connections and LLM budget; it must declare "needs connection type X" and "uses AI" so the tenant provisions them on install.
- **Payoff:** workflow-authored agents need **no Rust and no SDK** — anyone who can use the workflow builder can publish an agent. Likely the larger marketplace than hand-written Rust.

### 9.5 Three authoring tiers → three SDK/publish answers
| Author | Distribution | SDK need |
|---|---|---|
| First-party proprietary (v1) | private repo → build → overlay | **git-pin the SDK crates** (§4.2) |
| Third-party Rust authors | publish signed `.wasm`+`.meta` to registry | **published lean `runtara-agent-sdk` + public WIT + `cargo generate` template** — carve when this tier opens |
| Workflow authors | "publish as agent" from the builder | **none** — the compile target is the SDK |

### 9.6 A constraint that persists: new connection types still need core
A marketplace agent can only use connection/auth types the core already knows (the `HttpConnectionExtractor` lives in the trusted core, §2.6). A third-party integration agent needing a brand-new auth type isn't fully drop-in — either the core adds the type, or a **generic "HTTP with declared auth params" connection type** covers the common cases without per-service core code. Worth building the generic type to unblock most third-party integrations.

### 9.7 Suggested layering
- **v1 — Topology A:** proprietary agents (private repo, git-pin SDK, provisioning overlay). Ships the commercial split. (§5 Phases 0/3/4.)
- **v2 — Registry:** `agents.runtara.com` as an OCI-backed registry + index/search + signing + ABI gating; provisioning reconciles entitled agents from it. Generalizes the overlay.
- **v3 — Marketplace authoring:** (a) workflow-as-agent compile target (§9.4); (b) published lean SDK + template for third-party Rust authors (§9.5); (c) the commerce layer (publisher accounts, licensing/paid agents, ratings). Plus the generic connection type (§9.6).
