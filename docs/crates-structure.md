# Crate structure audit & restructuring plan

**Date:** 2026-07-25 · **Scope:** the 52-member Cargo workspace (`crates/`, `crates/agents/`), its
dependency graph, build times, test structure, and reusability · **Branch:** `main` @ `e249d1e9`

---

## Verdict

**The crate structure is not the problem.** 52 crates is a defensible granularity for this codebase,
the layering is mostly clean, and the WASM/native feature topology — the hardest part — is
deliberately and correctly designed in the places it matters most.

What is actually costing build time, maintainability and test confidence is four categories of
*defect* sitting on top of a sound structure:

1. **Two build-script bugs** that make every `cargo` invocation in the repo redundant work.
   A no-op `cargo check --workspace --all-targets` takes **5.0s and recompiles all 27 agent crates**
   because of a wrong relative path in a `rerun-if-changed` line.
2. **A dead dependency edge** (`runtara-workflows → runtara-agents`) that drags a C-library tail
   — vendored OpenSSL, libssh2, calamine, zip — into the workflow compiler, which never uses it.
3. **An unconditional `vendored` OpenSSL flag** justified by a musl build that does not exist.
   It is **121 of the 193 seconds (63%)** of a cold workspace check, and it sits on the critical path.
4. **CI that silently does not run what it appears to run** — ~28,700 LOC / ~1,400 Rust test
   functions are never executed, 78% of integration-test code is never linted, and a complete
   7-job frontend CI workflow has never run once because it is in the wrong directory.

Fixing those is mostly `S`-effort, touches no architecture, and is worth more than any crate split
in this document. **Do them first.** The genuine restructuring work is a much shorter list than the
crate count suggests, and it is in Phase 4.

One finding reprices the whole plan: **nothing is published to crates.io.** There is no
`cargo publish`, no `CARGO_REGISTRY_TOKEN`, and no crates.io reference in any of the five workflow
files or `scripts/release.sh` — the release pipeline builds GitHub release bundles and a Docker
image. So the usual "splitting a published crate is a breaking change" constraint **does not apply
here**. Crate splits and renames are internal refactors. The `publish = false` on three crates and
the `version = "8.6"` pins on path dependencies are unenforced ceremony.

---

## Implementation status (2026-07-25)

Phases 0–2 are **implemented and verified**. Measured before → after, same machine, fresh target dir:

| metric | before | after | change |
|---|---|---|---|
| cold `cargo check --workspace --all-targets` | 193s | **99.3s** | **−49%** |
| cold build CPU | 472s | 385s | −18% |
| total unit CPU (`--timings`) | ~1,700s | 1,214s | −29% |
| **no-op** `cargo check --workspace --all-targets` | **5.0s, 27 crates rebuilt** | **0.41s, 0 units** | **−92%** |
| `openssl-sys` build script | 121.14s | **1.84s** | −98% |
| `runtara-workflows` dependency closure | 194 pkgs | **116 pkgs** | −40% |
| native C tail in the compiler crate | 6 crates | **0** | eliminated |
| Rust unit tests running in CI | ~2,100 (14 crates skipped) | **3,508, all crates** | +67% |
| frontend tests running in CI | **0** | 1,105 (90 files) | — |

The critical path is now exactly what §1.3 predicted it would become once the C stack was removed —
the irreducible wasmtime/cranelift chain, ending in one `runtara-server` compile:

```
cranelift-srcgen → cranelift-assembler-x64-meta 3.58 → cranelift-assembler-x64 9.58
  → cranelift-codegen 11.69 → cranelift-frontend → wasmtime-internal-cranelift 1.48
  → wasmtime 4.97 → wiggle 4.55 → wasmtime-wasi 7.30 → wasmtime-wasi-http 1.38
  → runtara-component-host 0.60 → runtara-environment 3.12 → runtara-server 16.82  = 99.2s
```

`libssh2-sys` still builds from C (17.75s) but is no longer on the critical path.

**Landed:** A1, A2, A4, A6, B1, B2, B3, **B4**, B5, D1, D2, D3, plus MSRV (D9's `rust-version` half).

**B4 was verified and landed after an initial skip.** The first pass deferred it: `lib.rs:62`
instructed keeping the `runtara_ai as ai` re-export "until the AI Agent codegen is migrated to
dispatch through the `ai-tools` WIT agent", which looked like a live constraint. On investigation the
comment is accurate about the *old* codegen and stale about the present:

- `pub use runtara_ai as ai;` was the crate's **only** reference to `runtara_ai`. `direct_json.rs`
  parses the wire shapes by hand instead (its own comment at `:5138` says so).
- Nothing anywhere referenced `runtara_workflow_stdlib::ai` — not source, not a string literal, not
  a template. Only the two comments.
- The generated Rust that *did* use `ai::{completion, message, provider, types}` came from the
  native/components compile path the direct-only migration deleted. Surviving examples are untracked
  residue under `crates/runtara-workflows/.data/`, last written 2026-05-07.
- No current path emits Rust source or shells out to `cargo`; composition is in-process via wac-graph.

The TODO's other claim — that dropping the dep "would shrink workflow.wasm" — is **false, and could
not have been true**: the shipped component is built `--no-default-features --features
direct-component` (`build-agent-components.sh:147`), which leaves `sdk-runtime` off, so `runtara-ai`
was already absent from the artifact. Measured across the change:
`runtara_workflow_stdlib.wasm` 2,683,971 → 2,683,969 bytes (name-section noise). The gain is graph
clarity, not size.

Verified after removal: all four feature configurations the crate is really built with, fmt, gated
clippy, 3,508 workspace unit tests, a full 27-agent + 2-component wasm build, the entire
`direct_wasm_execute` suite (**159/159**), and specifically the **19 AI Agent tests** that execute
composed `workflow.wasm` against a hermetic LLM stub — single-shot, structured output, tool loops,
memory, breakpoints, durable replay, provider-error routing, retries, turn timeouts. All pass.
So B4 is an `S`, not the `M` the first pass re-rated it.

**Deliberately not done, with reasons:**
- **`--max-warnings=0` on frontend lint** — **not adopted.** 26 of the 34 warnings are
  `react-refresh/only-export-components`, an HMR-ergonomics rule whose fix is splitting 26 files.
  The 4 `react-hooks/exhaustive-deps` warnings are the ones worth acting on. The job gates on
  errors (0 today) instead; tighten once those 4 are fixed.
- **`[workspace.lints]`** — deferred. Encoding CI's `-D warnings` there makes every local
  `cargo build` fail on any warning, which is a working-style change to land deliberately, not as
  a side effect of this sweep. `rust-version` was added (51/51 crates inherit it).
- **A3** (`runtara-server` `build.rs` frontend/wasm pipeline) — still open; it is the one `M` item
  in Phase 1 and needs a full `embed-ui` build to verify. See R1 for the patch location.
- **Deleting the 3 remaining orphaned frontend workflows** — not done. `deploy-bunny.yml` (260
  lines) plus the two Claude Code workflows have no root equivalent, so deleting them destroys the
  only copy. `crates/runtara-server/frontend/.github/README.md` now explains what does and does not
  run, and what porting each would take.

Corrections to this plan that implementation forced:
- **The frontend `build` and `e2e-mocked` jobs DO need the Rust toolchain and wasm-pack** — the
  audit said they did not. `npm run build` fires `prebuild` → `build:wasm-validation` →
  `scripts/build-validation-wasm.mjs` → wasm-pack. Only lint/format/typecheck/unit/knip are
  Node-only. `.github/workflows/frontend.yml` splits the jobs on exactly that line.
- **Most frontend lint/format failures were in generated output**, not source. `eslint.config.js`
  ignored `src/wasm/validation` but not the vendored `src/wasm/runtara-report-dsl`, and
  `.prettierignore` covered neither. Excluding `src/wasm` from both (and from `knip.json`) makes
  `format:check` pass outright and drops lint to 0 errors — a config fix, not 17 file edits.
- **`runtara-object-store` was the one crate outside workspace inheritance**, with a `repository`
  URL pointing at `runtarahq/runtara-object-store`, a repo this crate has never lived in. Aligned.

---

## Method and confidence

Two evidence sources, kept distinct:

- **Direct measurement** (`cargo --timings` on a fresh target dir, `CARGO_LOG` fingerprint traces,
  `git log` churn, `crate::`-reference counts). These are reproducible — see the Appendix.
- **A 12-agent audit** across 8 dimensions, then 3 adversarial verification lenses
  (build-correctness, WASM/feature-topology, migration-cost) and a completeness critic.
  75 findings; the lenses refuted 2, flagged ~10 duplicate triplets, and produced the sequencing
  constraints in Phase 4.

Every claim marked **[verified]** below I reproduced myself. Claims from the audit that I did not
independently reproduce are marked **[audit]** and carry the agent's evidence citation. Two audit
findings I checked and **refuted** — they are recorded in Part 5 rather than silently dropped,
because a plan is more useful when it says what *not* to do.

Measurement caveat stated up front: my baseline (193s) is a clean run; my `OPENSSL_NO_VENDOR`
comparison run and the audit's numbers (379.8s) were taken while other cargo jobs were running, so
their **wall-clock** figures are inflated. Per-unit durations and the critical-path *shape* from
`--timings` are unaffected by that and are what I rely on.

---

## Part 1 — What the numbers say

### 1.1 The cold build is 79% third-party and barely parallel

Cold `cargo check --workspace --all-targets`, 16 cores, fresh target dir: **193s wall, 472s CPU
= 2.4× parallelism.** First-party code is **362s of ~1,700s total CPU (21%)**. **[verified]**

That ratio is the single most important number in this document: **no rearrangement of first-party
crates can fix a build that is 79% dependency compilation and serialized on C libraries.**

### 1.2 The critical path is C libraries, then wasmtime, then one big crate

Baseline: **[verified]**

```
libc → cc → openssl-src → openssl-sys[build-script] 121.14s
  → libssh2-sys[build-script] → ssh2 → runtara-agents → runtara-connections → runtara-server 20.86s
```
14 units, 161s of the 193s wall. `openssl-sys`'s build script starts at t=42.4 and ends at t=163.6;
`runtara-agents` cannot start until t=168.1. **One unit occupies the machine for 121 seconds.**

Setting `OPENSSL_NO_VENDOR=1` drops that unit to **2.97s** — and exposes what was hidden behind it:
`libssh2-sys[build-script]` at **63.50s** (libssh2 also builds from C source), after which the
critical path becomes the wasmtime/cranelift chain: **[verified]**

```
cranelift-srcgen → cranelift-assembler-x64-meta 10.16 → cranelift-codegen-meta 6.70
  → cranelift-codegen ×3 (1.22 + 8.76 + 11.99) → cranelift-native 5.92
  → wasmtime-internal-cranelift 8.87 → wasmtime 8.28 → wiggle 4.77 → wasmtime-wasi 12.14
  → wasmtime-wasi-http 3.78 → runtara-component-host 1.26 → runtara-environment 6.80
  → runtara-server 36.98
```

So there are **two independent native-toolchain serializations**, both terminating in a single large
`runtara-server` compile:
- **(a)** the C crypto/ssh stack, entered via `runtara-agents/native → ssh2`
- **(b)** the wasmtime+cranelift stack, entered via `runtara-component-host`

(b) is irreducible — the server embeds wasmtime by design. (a) is entirely removable.

### 1.3 No cargo invocation in this repo is ever a no-op

A true no-op `cargo check --workspace --all-targets` takes **5.0s and recompiles all 27 agent
crates.** Cargo's own fingerprint log gives the reason verbatim: **[verified]**

```
stale: missing ".../crates/agents/runtara-agent-xml/../runtara-agent-wit/templates/agent.wit.in"
dirty: FsStatusOutdated(StaleItem(MissingFile { path: ".../runtara-agent-xml/../runtara-agent-wit/..." }))
```

All 27 `crates/agents/*/build.rs` are byte-identical (`md5 -q … | sort -u` → 1). Line 13 correctly
uses `include_str!("../../runtara-agent-wit/…")`; line 39 emits
`cargo:rerun-if-changed=../runtara-agent-wit/…` — one `../` short. Relative to `CARGO_MANIFEST_DIR`
that resolves to `crates/agents/runtara-agent-wit/…`, which does not exist. A `rerun-if-changed`
path that does not exist makes cargo treat the build script as permanently dirty.

This tax is paid by every `cargo check`, `clippy`, `test -p …`, and every rust-analyzer save cycle.
The audit measured the fix in a throwaway copy: **5.2s / 29 units → 0.49s / 0 units.** **[audit]**

### 1.4 `runtara-dsl` compiles twice

Two distinct feature sets, 4 units, 17.7s: `[default, fs, json-schema, utoipa]` (the unified normal
-dep set) and `[default, json-schema]`. The second exists because `runtara-server` declares
`runtara-dsl` as a `[build-dependencies]`, and resolver-2 keeps build-deps in their own feature
context. **[verified]**

### 1.5 Feature unification defeats declared opt-outs — the load-bearing principle

Three crates declare their intent about `runtara-agents`: **[verified]**

| crate | declares | wants |
|---|---|---|
| `runtara-connections` | `{ default-features = false }` | no `native` |
| `runtara-workflows` | `{ default-features = false }` | no `native` |
| `runtara-server` | `{ default-features = false, features = ["native"] }` | `native` |

All three build for the same target in one invocation, so cargo unifies `runtara-agents` to
`native` **once**. Connections and workflows transitively wait on ssh2 → libssh2 → OpenSSL despite
opting out — visible directly in the §1.2 critical path.

> **Features unify across a workspace build; crate boundaries do not.**
> A `default-features = false` declaration is not an isolation mechanism. Only a separate crate is.

This is why several fixes below are crate splits rather than feature changes, and it is the single
most useful idea to carry into future design here.

### 1.6 Churn: where change actually lands

Commits touching each path, last 90 days: **[verified]**

`server 476` · `workflows 169` · `workflow-stdlib 93` · `agents 74` · `environment 65` ·
`connections 63` · `dsl 62` · `core 44` · `sdk 42` · `report-dsl 32` · `object-store 21` ·
`agent-ai-tools 20` · then every remaining agent crate ≤ 9.

`runtara-server` absorbs **~5 commits/day**, each costing a 20–37s recompile of a 95k-LOC crate.
That is the strongest first-party case for decomposition in the workspace — and it is a *dev-loop*
case, not a cold-build case.

Within `runtara-dsl` (all-time | 90d): `schema_types 56|28` · `agent_meta 44|22` · `lib 30|12` ·
`form/mod 11|11` · everything else ≤ 3 in 90 days. **[verified]**

This **refutes the intuitive "split `runtara-dsl` into stable-types + churny-rest"** proposal: the
churn is concentrated in `schema_types.rs` and `agent_meta.rs`, which is *exactly* what the 27 agent
crates consume. A stable/churny split leaves the agents on the churny side. Any `runtara-dsl` split
must run the *other* direction — move `form/`, `spec/`, `condition_eval` **out**, so they stop
rebuilding when `agent_meta` changes.

---

## Part 2 — What is healthy (do not touch)

A restructuring plan that churns working code is a bad plan. These were examined and should be left
alone:

- **The WASM/native feature topology in the places it matters.** `runtara-http` having *no* default
  backend is deliberate and documented. The 27 agent crates target-gate it correctly
  (`[target.'cfg(not(target_family = "wasm"))'.dependencies]` → `native`,
  `[target.'cfg(target_family = "wasm")'.dependencies]` → `wasi`). **[verified]**
- **`runtara-validation-wasm`** uses `default-features = false, features = ["wasm-js"]` on
  `runtara-workflows`, correctly keeping the compiler toolchain out of a browser bundle. **[verified]**
- **`runtara-report-dsl → runtara-object-store`** is optional behind `aggregate`, and
  `Cargo.toml:24` documents exactly why (object-store "pulls sqlx + tokio and isn't WASM-friendly").
  This is deliberate, correct design. **[verified]**
- **`runtara-http`'s `wasi = "=0.14.1"` exact pin** — the comment explains the nominal-type
  resolution failure it prevents. Do not touch it; treat it as load-bearing. **[verified]**
- **`spikes/*`** are deliberately *outside* `[workspace.members]` with their own lockfiles and an
  explanatory header. Correct — adding them as members would unify wasmtime 46 into the workspace
  graph and reintroduce the toolchain skew they exist to isolate. **[audit]**
- **`runtara-agent-encoding`** (341 LOC, 10 tests) — the right size and shape for a shared
  vocabulary crate; the model other consolidations should copy.
- **`runtara-object-store`** (13.9k LOC, zero first-party deps, 303 inline tests) — the best-tested
  crate in the workspace and the correct owner of the SQL tier.
- **`runtara-component-host`**, **`runtara-text-parser`**, **`runtara-workflow-runtime`**,
  **`runtara-sdk-macros`**, **`runtara-ai`** — all appropriately sized and scoped.
- **Migrations** — three migrator dirs, each owned by the crate that owns its tables, embedded via
  `sqlx::migrate!`. No shared numbering namespace, no collision. **[audit]**
- **27 agent crates is the right granularity.** Independent versioning and distribution for a
  marketplace favours separate crates; merging them would fight that goal. The problem is lockstep
  versioning and duplicated scaffolding, not the crate count. **[audit]**
- **Do not split the large agent `lib.rs` files as a project of its own** (shopify 5,841 / text
  3,588 / hubspot 3,386). The crate, not the file, is cargo's unit of both parallelism and
  recompilation, so file splitting buys **zero** build time. Split them opportunistically when
  editing, on maintainability grounds only.

Also: **`build.rs` proliferation is not a problem.** There is exactly one non-agent build script
(`runtara-server`). The 27 agent scripts are one duplicated file with one bug. **[audit]**

---

## Part 3 — Findings by root cause

### A. Build defects (not structural)

| # | Finding | Goal | Effort |
|---|---|---|---|
| A1 | **27 agent `build.rs` have a wrong `rerun-if-changed` path** → no cargo invocation is ever a no-op (5.0s + 27 crate recompiles, every time). **[verified §1.3]** | build | **S** |
| A2 | **`vendored` OpenSSL is unconditional** and its justifying musl target does not exist. 121s = 63% of cold wall, on the critical path. **[verified §1.2, §1.4]** | build | **S** |
| A3 | **`runtara-server`'s `build.rs` re-runs `wasm-pack` + `npm run build` on every invocation.** 37.98s unit in the cold build — second largest after openssl-sys; measured 38.7s → 22.6s → 22.4s across three consecutive no-change runs, printing "REBUILDING FRONTEND DIST" each time. **Two independent causes, both live in committed code:** (i) `build.rs:256` sorts with `files.sort()` — `PathBuf`'s component-wise `Ord` — while `build-validation-wasm.mjs:97` uses JS `files.sort()` (whole-string). Both write the same `runtara_validation.fingerprint`, and FNV-1a is order-sensitive, so **the two fingerprints can never agree and each side always treats the other's as stale.** The input list at `build.rs:229-249` includes `crates/runtara-workflows/src`, which contains the one sibling pair in the tree that makes the orders differ (`compile.rs` vs `compile/`). Proven empirically: `PathBuf::sort` → `[compile/abi.rs, compile.rs]`; both string sorts → `[compile.rs, compile/abi.rs]`. (ii) `cargo:rerun-if-changed={dist}` on a directory the script itself writes. **[verified]** | build | **M** |
| A4 | **`build-agent-components.sh` runs 27 sequential `cargo` invocations**; CI's `test-agents` loop does the same 27× (`ci.yml:410-414`). Batching → measured 4.1× faster. **[audit]** | build | **S** |
| A5 | `[profile.dev.package."*"] opt-level = 2` is workspace-wide but was motivated by the wasmtime/cranelift-driven test suites specifically. Scope it to those packages. **[audit]** — but see the tension in Phase 4 note. | build | **S** |
| A6 | `default-members = ["crates/runtara-core"]` — a bare `cargo build` builds only `runtara-core`, which is nobody's default task. **[verified]** | build/DX | **S** |

**A2's concrete blocker, which no single finding named:** `docker/Dockerfile` installs only
`ca-certificates` and `curl` on `ubuntu:22.04`. Removing `vendored` means `libssh2-sys` links
against system OpenSSL, so **`libssl3` must be added to the image** and `libssl-dev` must be present
on build runners (GitHub's ubuntu images have it; macOS devs need `brew install openssl@3`). Keep it
as an opt-in `vendored-openssl` feature rather than deleting the dep, so a static-linking lane
remains available and the change is reversible. **[audit]**

### B. Dead edges (deletions, no design required)

| # | Finding | Effort |
|---|---|---|
| B1 | **`runtara-workflows → runtara-agents` is entirely dead.** `runtara_agents` appears in the crate only in `Cargo.toml` (dep at `:42`, feature forwards `native-agents` `:25` and `wasm-js` `:26`) and one prose comment. Zero references in any `.rs` under `src/` or `tests/`. Yet `default = ["compiler", "native-agents"]` means the compiler crate's **default** features pull `runtara-agents/native` → ssh2 → vendored OpenSSL. `cargo tree -p runtara-workflows -e normal,build`: **194 packages; 131 with `--no-default-features --features compiler`.** Also drags `runtara-agents` + `strum` into the browser validation WASM. **[verified]** | **S** |
| B2 | `runtara-environment → runtara-dsl` — zero occurrences of `runtara_dsl` in `environment/src/`. **[verified]** | **S** |
| B3 | `runtara-environment` dev-depends on `runtara-workflows`, unused. **[audit]** | **S** |
| B4 | `runtara-workflow-stdlib → runtara-ai` is a bare `pub use runtara_ai as ai;` re-export; `direct_json.rs` explicitly parses the shapes by hand instead. The crate's own `Cargo.toml` carries a TODO to remove it. **[verified]** | **S** |
| B5 | `runtara-workflows` declares `tracing-subscriber` in **both** `[dependencies]` and `[dev-dependencies]`; the normal one is unused. Plus 7 more unused declared deps workspace-wide, including `wiremock` in a crate with no wiremock tests. **[audit]** | **S** |
| B6 | `runtara-sdk`'s **default** `embedded` feature makes an SDK depend on the engine (`runtara-core` + tokio) for a dead code path. **[audit]** *(caveat: this does not shrink `cargo test -p runtara-sdk`, because `runtara-core` is also a dev-dep for the embedded heartbeat tests — the payoff is graph clarity, not that job's time.)* | **S** |

**B1 is the highest cost/benefit item in this entire document** — a three-line Cargo.toml deletion
that removes the native C tail from the compiler's dependency closure. All three verification lenses
independently ranked it first or second.

**B1 must precede any attempt to build `runtara-workflows` without default features:**
`cargo check -p runtara-workflows --no-default-features` fails *today* with `runtara-http`'s
"requires exactly one backend feature" `compile_error!`, purely because of this dead edge. **[audit]**

### C. Dependency hygiene

| # | Finding | Effort |
|---|---|---|
| C1 | **Hoist ~40 deps into `[workspace.dependencies]`** (currently only 10). **15 are declared at genuinely divergent versions**: `thiserror` 1.0 (report-dsl, server) vs 2 vs 2.0 (9 crates); `regex` 1 / 1.10 / 1.11; `rand` 0.8 / 0.9; `utoipa` 5 / 5.3; `sqlx` 0.8 / 0.8.6 with **four** different TLS/feature combos; `wit-parser` 0.246 / 0.247; `darling` 0.20 / 0.23; plus serde/serde_json/chrono/base64/sha2/syn/quote/proc-macro2 spelling drift. **`wit-bindgen = "0.58"` is declared in 30 separate manifests** — a toolchain bump is a 30-file edit. **[audit]** | **M** |
| C2 | **Three TLS stacks and four root stores in one binary.** `cargo tree -e features -i sqlx-core` confirms sqlx-core compiles with `_tls-native-tls` **and** `_tls-rustls-ring-webpki` simultaneously. Same split in reqwest 0.12 (`default-tls` from connections/server vs `rustls-tls` from component-host/management-sdk). Root stores: `webpki-roots` 0.26 **and** 1.0; `rustls-native-certs` 0.7 **and** 0.8; `security-framework` 2 **and** 3. Standardize on `rustls-ring-webpki`. **[audit]** | **L** |
| C3 | `opentelemetry-otlp` default features drag a **second reqwest (0.13.4)** that nothing calls. **[audit]** | **S** |
| C4 | `runtara-dsl`'s form validators put `regex` in every agent WASM build. **[audit]** | **M** |
| C5 | `jsonschema` pulls a blocking HTTP client to validate in-memory schemas. **[audit]** | **S** |

**C1 sequencing:** hoisting is mechanical and low-risk, but it **bundles three behavioural changes**
that must be separate commits with their own verification: `thiserror` 1→2, `rand` 0.8→0.9, and the
C2 TLS unification (which carries a root-store behaviour change). Do the version-string
normalization as one commit; do those three as three more.

### D. CI and test-structure gaps

These are the findings with the largest gap between *apparent* and *actual* coverage.

| # | Finding | Effort |
|---|---|---|
| D1 | **CI enumerates test crates by `-p`; no job runs `cargo test --workspace`.** Never executed: `runtara-dsl` (253 test fns), `workflow-stdlib` (190), `connections` (140+30), `management-sdk` (139), `report-dsl` (87), `text-parser` (50), `sdk-macros` (31), `ai` (28), `validation-wasm` (20), `http` (10), `agent-encoding` (10), `workflow-runtime` (9), `workflow-wit` (5). Narrowed away: `object-store`'s 303 inline fns (`--test integration` excludes the lib target), `runtara-agents`' `tests/` (16 fns, `--lib`). **≈28,700 test LOC / ≈1,400 test fns that compile and never run.** A new crate defaults to untested forever. `runtara-agent-encoding` is missed twice over — the loop globs `crates/agents/runtara-agent-*` but that crate lives at `crates/runtara-agent-encoding`, outside the glob, contradicting the loop's own comment. **[audit]** | **S** |
| D2 | **The lint job skips 78% of test code.** `clippy --workspace --all-targets` without the gate features silently drops every `[[test]]` with `required-features`. **25,392 of 32,707 LOC in `tests/` are neither linted nor built** — including `direct_wasm_execute.rs` (11,448), all of environment's 6,967, object-store's 4,183. `-D warnings` is enforced on 22% of integration-test code. All these gates are empty marker features, so enabling them changes no produced binary. **[audit]** | **S** |
| D3 | **The frontend's 7-job CI workflow has never run.** `crates/runtara-server/frontend/.github/workflows/pr-checks.yml` is complete and git-tracked, but GitHub only discovers workflows at the **repo root**. Consequence: **36,616 LOC of frontend tests never run** (90 vitest files / 1,057 cases / 21,743 LOC, plus 64 Playwright specs / 14,873 LOC). eslint never runs; vitest never runs; `knip` never runs. 140,702 LOC of TS/TSX is gated by type-checking alone. Four orphaned workflow files sit in that directory. **[audit]** | **S** |
| D4 | **7 agent crates, ~16,500 LOC, have zero tests** — and CI's per-crate `--lib` job passes vacuously on each: shopify 5,841 · hubspot 3,386 · stripe 2,189 · s3-storage 1,428 · utils 1,312 · http 1,204 · xlsx 562 · mailgun 556. Near-zero: openai 1,692→1, slack 975→2, sftp 653→2. **[verified]** | **M** |
| D5 | **No shared test-support crate.** testcontainers Postgres scaffolding is copy-pasted **9 times** across core/server/environment/object-store. **[audit]** | **M** |
| D6 | `runtara-connections` has `default = []`, **zero `[[test]]` blocks**, and 7 of 11 test files need Docker — so `cargo test --workspace` hard-fails without Docker today. **This blocks D1** and must land first. **[audit]** | **S** |
| D7 | `runtara-connections` tests assert against **hand-written DDL that has already drifted** from the production migrations. **[audit]** | **M** |
| D8 | `e2e/` never runs in CI, and 9 of 18 scripts are orphaned from `run_all.sh`. **[audit]** | **M** |
| D9 | No **`[workspace.lints]`** and no **`rust-version`/MSRV** anywhere. Lint policy exists only inside the CI command line, so a local `cargo clippy` does not reproduce CI. **[verified]** | **S** |
| D10 | The 8,827-line generated API client `RuntaraRuntimeApi.ts` has **no drift gate** — three generator scripts exist, none runs in CI. 6 commits have touched `src/api` since the client was last regenerated. `RuntaraManagementApi.ts` is worse: last touched 2026-04-19, and wraps endpoints since deleted. **[audit]** | **S** |
| D11 | The **910 KB `runtara-report-dsl` WASM bundle is hand-vendored into git with no producer** and is **6 commits stale** — the browser's copy of the minijinja engine and condition evaluator does not know about the `file_upload` block type or staged views the server already ships. The only two `wasm-pack` invocations in the repo both build `runtara-validation-wasm`. **[audit]** | **M** |
| D12 | **`sccache` is configured where it helps least.** `scripts/clean-target.sh:3` claims sccache is "configured as rustc-wrapper in `.cargo/config.toml`" and uses that to justify an unattended `cargo clean` at 15 GiB — but `.cargo/config.toml` has no `rustc-wrapper` at all. CI uses `CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER` in only the two release workflows, which by definition wraps **only workspace members**, leaving all ~700 dependency crates — i.e. the 79% from §1.1 — uncached. `ci.yml`'s hot `lint` and `build` jobs have no sccache at all. **[audit]** | **S** |

### E. Genuine structural problems

These are the real restructuring items. All require a design decision, and several conflict with
each other — see Phase 4.

**E1 — Dissolve `runtara-agents`; it is three unrelated crates wearing one name.** (`L`)

7,056 LOC with disjoint consumers: **[verified]**
- the connection-type/extractor registry (~2,700 LOC) — consumed by `runtara-connections` (17 call
  sites) and one server call to `extractors::augment_catalog`
- native-only C workers `compression`/`sftp`/`xlsx` (2,359 LOC, `#[cfg(feature = "native")]`) —
  consumed only by `runtara-server`
- `s3_client.rs` (424 LOC) — not an agent at all; consumed only by the server's file-storage service

Meanwhile `types.rs` (987 LOC, `AgentError`/`FileData`) and `connections.rs` (`RawConnection`) have
**zero external consumers**, because the enclosing crate drags tracing/ssh2/calamine into the tree.
So **`RawConnection` is redefined in 20 agent crates and `AgentError` in 23**, each with a comment
naming `runtara_agents::types` as the thing it duplicates. **[audit]**

The one real coupling: `static_registry.rs` is a **hand-maintained** `CAPABILITY_REGISTRATIONS`
slice whose entries name `crate::compression::*`, `crate::sftp::*`, `crate::xlsx::*` behind
`#[cfg(feature = "native")]`. `registry.rs` itself never names a native agent — it only reads the
slice. **[verified]** So the split needs registration inverted (consumer composes the slice, or the
native crate exports its own) — contained, not a rewrite.

This is the fix that §1.5 demands: because features unify but crates don't, only a crate boundary
actually removes ssh2/OpenSSL from `runtara-connections`' closure.

**E2 — Extract the validation half of `runtara-workflows` into a leaf crate.** (`L`)

`validation.rs` (13,423 LOC) imports only `crate::dependency_analysis`, `runtara_dsl`, and `std`.
The same holds for `input_validation.rs`, `schema_fields_validation.rs`, `workflow_features.rs`,
`dependency_analysis.rs`. The dependency between halves is strictly one-directional — the emitter
imports the validation modules, never the reverse. Yet it all lives in a 58k-LOC crate whose
`compiler` feature pulls 7 wasm-toolchain crates. Consumers wanting validation *without* the emitter:
`runtara-validation-wasm` (the browser validator) and 2 of the server's 3 uses. **[audit]**

New leaf `runtara-workflow-validation`, deps exactly `runtara-dsl + serde + serde_json + minijinja`.
Keep `runtara-workflows`' existing `pub use` re-exports verbatim so its API is unchanged.
**Requires B1 first** (see B1's note).

**E3 — Extract the MCP server into `runtara-mcp`.** (`L`) — the cleanest seam in the codebase.

`mcp/` is 13,557 LOC with inbound coupling of **exactly one line** (`use crate::mcp;` at
`server.rs:22`) through a single constructor. It does not reach into the crate by function call:
**114 in-process HTTP calls via an injected `axum::Router` versus exactly 1 `sqlx::query`** in all of
`mcp/`. Only 6 call sites bypass the router. `schemars` is used by `mcp/` and nowhere else; `rmcp`
leaks outside it in 2 places. The `object_store_manager` field is threaded through 5 layers and
never read. **[audit]** My own coupling count agrees: ~18 outbound `crate::` refs, 2 inbound. **[verified]**

Invert the 6 bypass sites into constructor-injected traits and `runtara-mcp` needs only
sqlx + axum + rmcp + runtara-dsl + runtara-report-dsl — **no `runtara-server-core` split required
first.** That makes E3 independent of the harder server decomposition.

**E4 — A shared host/guest manifest-contract crate.** (`L`)

The direct-workflow manifest is defined **twice** — host-side in `runtara-workflows`, guest-side in
`runtara-workflow-stdlib/direct_json.rs` — with no version negotiation. **[audit]** This is the
structural cause of a known class of runtime failure in this repo: a step type present in the
emitter but missing from `direct_json.rs`'s debug builders crashes at runtime rather than failing to
compile. A shared contract crate turns that into a compile error.

**E5 — Break the `runtara-report-dsl ↔ runtara-object-store` inversion.** (`M`)
There are **two identical `Condition` types** inside report-dsl's own tree. **[audit]**
**Must precede** any move of the query planner (its `AggregateRequest`/`AggregateSpec` come from
object-store via the `aggregate` feature, which pulls sqlx+tokio and cannot compile for
wasm32-unknown-unknown).

**E6 — `runtara-environment` reads `runtara-core`'s `instances` table with raw SQL** while also
holding core's `Persistence` trait. (`M`) Two access paths to one table across a crate boundary. **[audit]**

**E7 — `runtara-server` decomposition.** (`XL`) — the largest *maintainability* item, and the one to
approach last and carefully.

My coupling measurements: **[verified]**

| subsystem | LOC | outbound `crate::` refs | inbound |
|---|---|---|---|
| `mcp/` | 13,557 | ~18 | 2 |
| `channels/` | 4,480 | 21 | 1 |
| `middleware/`+`auth/`+`authz/` | 6,371 | mostly internal; 2 out | — |
| `api/services/reports*` | 4,016 | 61 (**34 to `api::dto`**) | — |
| `api/services/` | 20,150 | **73 to `api::dto`** + 43 | — |
| `workers/` | 4,976 | 22 to `valkey::*`, 14 `runtime_client`, 8 `api::dto` | — |

**`api/dto` (4,204 LOC) is the gravitational centre.** Every extraction candidate points inward at
`api::dto` + `api::repositories` + `api::services`. So "extract `mcp/`" is not the first move —
except that E3 sidesteps this entirely by inverting its 6 bypass sites. Beyond E3, the enabling move
is extracting the shared DTO/domain layer, and `middleware`+`auth`+`authz` (6,371 LOC, nearly
self-contained) is the next cleanest unit.

**Also here (smaller):** the pure workflow-graph validators in `api/utils` should move so the browser
can run them; the `ApiDoc` derive grows unbounded; MCP re-derives reference shapes from JSON instead
of using `runtara-dsl`'s typed model; `runtara-management-sdk` re-declares dsl/core vocabulary with
a snake_case/camelCase wire divergence. **[audit]**

### F. Outside the crate graph

| # | Finding | Effort |
|---|---|---|
| F1 | **`crates/runtara-server/frontend` is a 140k-LOC repo-inside-a-crate** with its own `.github/`, `package.json`, `knip.json`, e2e suite, and no boundary. Decide whether that is its home. **[audit]** | **M** |
| F2 | 3.2 GB of `.data/workflow-builds-components` is orphaned output of the deleted components-mode compile path — 50 complete generated Cargo projects, last written 2026-05-31. No cleanup script covers `.data` (only `target/`). Untracked, so deleting is local hygiene. **[audit]** | **S** |
| F3 | `prototypes/`, `examples/`, `runtara-on-runtara/` are referenced by nothing; `runtara-on-runtara/` is an **empty untracked directory**. Mark archives as archives. **[audit]** | **S** |
| F4 | `recharts` is a `devDependency` but four production modules import it. **[audit]** | **S** |
| F5 | **32 of 52 crates have no README**, including all 27 agent crates. Only 3 crates set `publish = false`; the other 49 carry publish-by-default as unenforced ceremony (nothing publishes — see Verdict). **[verified]** | **S** |
| F6 | `[profile.dev]` debuginfo tuning + a darwin `lld` entry (the three Linux targets already have one): measured **40% smaller `target/`** for 9–22% less wall. Worth it mainly because `clean-target.sh` triggers a cold rebuild at 15 GiB. **[audit]** | **S** |

---

## Part 4 — The plan

Sequenced so that each phase's measurements are trustworthy and no phase invalidates a prior one.

### Phase 0 — Make measurement possible (do this before anything else)

**A1** — fix `../` → `../../` in all 27 agent `build.rs`.

Non-negotiably first. Until it lands, **no cargo invocation in the repo is a no-op**, so every
incremental measurement in Phases 1–5 is confounded. All three verification lenses said this
independently.

Consider going further and **deleting the 27 build scripts**: `crates/agents/*/wit/agent.wit` is
already committed, the content is a deterministic function of the crate name, and a single test in
`runtara-agent-wit` can assert each committed file matches template+id. That removes 27 duplicated
files instead of fixing a typo in each. (Path fix = zero risk; deletion needs the check-mode test to
avoid silent drift.)

**Exit criterion:** two consecutive `cargo check --workspace --all-targets` runs, second one
≤ 1s / 0 units.

### Phase 1 — Delete dead weight (all `S`, no design decisions)

Order within the phase does not matter except where noted.

1. **B1** — delete `runtara-workflows → runtara-agents` (dep + 2 feature forwards), updating
   `runtara-server`'s and `runtara-validation-wasm`'s feature selections. *Highest cost/benefit item
   in the plan.* **Blocks E2.**
2. **A2** — split `runtara-agents`' `native` feature; move `openssl` behind an opt-in
   `vendored-openssl` that only a static-linking lane enables. **Add `libssl3` to
   `docker/Dockerfile`** in the same commit — that is the step that makes it not break the image.
3. **B2, B3, B4, B5, B6** — remaining dead edges and unused deps, as one commit each.
4. **A3** — the `runtara-server` `build.rs` frontend/WASM pipeline (`M`, the one non-`S` item here;
   include it because it costs 22–38s on *every* server build, including both hot CI jobs).
   Preferred fix: take the frontend/wasm-pack orchestration out of `build.rs` entirely — `npm run
   build` already owns the prebuild wasm step, and CI's lint and build jobs already invoke it — and
   have `embed-ui` merely *assert* that `frontend/dist` and `frontend/src/wasm/validation` exist,
   failing with today's helpful message otherwise. That deletes both causes and the duplicated
   fingerprint machinery at once. If the guard must stay, then at minimum keep exactly one
   fingerprint implementation and stop emitting `rerun-if-changed` on a directory the script writes.
   Note the input list at `build.rs:229-249` is byte-duplicated in `build-validation-wasm.mjs:39-54`
   — narrowing one without the other re-creates the divergence.
5. **A4** — batch `build-agent-components.sh`'s 27 cargo invocations into one; same for
   `ci.yml:410-414`.
6. **A6** — fix or drop `default-members`.
7. **F2, F3** — reclaim 3.2 GB; mark archives.

**Then re-measure the cold build and the no-op floor.** Phases 2+ should be re-priced against the
new numbers, because Phase 1 removes the three largest cache misses and changes what caching is
worth (this is why **D12/sccache comes after**, not before).

Expected: cold check ~193s → ~100–120s (removing 121s of OpenSSL from the critical path, with
libssh2's 63s and the cranelift chain becoming the new bound); no-op 5.0s → ~0.5s.

### Phase 2 — Make CI test and lint what it appears to (all `S`, highest confidence-per-hour)

1. **D6** first — give `runtara-connections` proper `[[test]]` gating, or `cargo test --workspace`
   hard-fails without Docker. **Blocks D1.**
2. **D1** — replace per-crate `-p` enumeration with `--workspace`; fix the
   `crates/agents/runtara-agent-*` glob that misses `runtara-agent-encoding`. Unlocks ~1,400
   existing test functions.
3. **D2** — add the gate features to the clippy and build jobs. These are empty marker features:
   no produced binary changes, and 78% more test code gets type-checked.
4. **D3** — port `pr-checks.yml` to `.github/workflows/frontend.yml` with `working-directory` set;
   delete the four orphaned files. Add `--max-warnings=0` (eslint's `no-unused-vars` is `warn` and
   the script has no flag, so the job would otherwise pass while warning). Keep it a separate job —
   it needs no Rust toolchain and parallelizes with the cargo build.
5. **D9** — add `[workspace.lints]` and `rust-version` so local clippy reproduces CI.
6. **D10** — add the OpenAPI/API-client drift gate (4 lines; the offline generator already exists).
   Delete or gate the stale `RuntaraManagementApi.ts`.
7. **D12** — fix the false sccache claim in `clean-target.sh:3`; switch the release workflows from
   `CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER` to `RUSTC_WRAPPER` so the 79% dependency tail is actually
   cached; evaluate adding it to `ci.yml`'s hot jobs.

Phase 2 will surface pre-existing failures in code that has never been linted or run. Budget for
that; it is the point.

### Phase 3 — Dependency consolidation

1. **C1a** — mechanical hoist of ~40 deps into `[workspace.dependencies]`, versions unchanged where
   they already agree. One commit.
2. **C1b/C1c/C1d** — the three behavioural bumps as three separate commits: `thiserror` 1→2,
   `rand` 0.8→0.9, and **C2** TLS unification onto `rustls-ring-webpki` (watch the root-store
   change; verify against the valkey-TLS suite, which historically never passed).
3. **C3, C5** — trim `opentelemetry-otlp` and `jsonschema` default features.

### Phase 4 — Structural work (needs decisions; see Part 6 first)

Recommended order, driven by the prerequisite chains the verification lenses found:

1. **E1** (dissolve `runtara-agents`) — but **resolve Decision 1 first**; three findings proposed
   three different homes for the shared agent wire vocabulary.
2. **E5** (report-dsl ↔ object-store inversion) — **must precede** any query-planner move.
3. **E2** (extract `runtara-workflow-validation`) — **requires B1**.
4. **E3** (extract `runtara-mcp`) — independent of E7 if the 6 bypass sites are inverted. The best
   ratio of value to risk in this phase.
5. **E4** (shared host/guest manifest contract) — **resolve Decision 2 first** (two findings put the
   canonical step-type table in different crates).
6. **E6**, then the smaller E7 items.
7. **E7 proper** (`runtara-server` decomposition) — last, and only if the dev-loop pain justifies it
   after Phases 0–3. Enabling move is the `api::dto`/domain layer, not the visible leaf subsystems.

**Note on A5 (`profile.dev` scoping):** it is in tension with Phases 2/5 — narrowing
`opt-level = 2` slows exactly the wasmtime/Cranelift-driven suites that D2 wants linted and D4/D5
want run more often. Measure against `cargo test -p runtara-workflows --features
direct-wasm-integration-tests`, not `cargo check`, before committing to it.

**Do not** split `validation.rs`'s or `direct_json.rs`'s inline test modules as a build optimization
— see Part 5.R3.

### Phase 5 — Test depth

1. **D4 + a shared mock-HTTP testkit** for the `runtara-http` abstraction — the enabler for testing
   8 agent crates with zero coverage, the 3 largest among them. Repo policy: tests live in the
   component crates or full e2e; no static registry.
2. **D5** — extract `runtara-test-support` (dev-dependency only) for the 9× duplicated Postgres
   container scaffolding.
3. **D7** — replace `runtara-connections`' hand-written DDL with the production migrations.
4. **D11** — make the report-dsl WASM bundle a build output rather than a stale commit; resolve
   **Decision 3** while doing it.
5. Split `direct_wasm_execute.rs` (11,448 LOC / 117 tests) into ~4 `[[test]]` targets **after**
   extracting its 1,764-line harness — separate binaries also run in parallel.
6. **D8** — reconnect `e2e/`'s 9 orphaned scripts; move the four that belong one layer down.

---

## Part 5 — Rejected proposals

Recorded so they are not re-proposed. Two are refutations of audit findings; one refutes an
intuition I started with.

**R1 — (withdrawn) "The two wasm fingerprint implementations diverge."** An earlier draft of this
document listed that finding as *refuted*, on the grounds that `build.rs` already sorted by path
string and both files carried comments explaining why. **That was my error, and the finding is
correct.** One of the audit subagents had silently fixed the bug in the working tree — and written
those very comments — before I read the file, so I mistook its uncommitted fix for pre-existing
code. `git show HEAD:crates/runtara-server/build.rs` is `files.sort()` with no such comment.
The divergence is live in committed code and is now **A3(i)**, empirically proven.

The subagent's edit has been reverted (this audit produces a plan, not changes). The patch is
preserved at `/private/tmp/.../scratchpad/A3-fingerprint-fix.patch` if useful as a starting point,
but A3 should be implemented deliberately in Phase 1 with a full build to verify — the correct fix
is arguably to remove the duplicated fingerprint machinery altogether rather than to keep two
implementations in sync.

*Process note worth keeping:* when verifying a claim about source, check `git show HEAD:<path>`, not
just the working tree. A working tree can contain changes made during the investigation itself.

**R2 — "Stop linking 27 host rlibs into `runtara-agent-bundle-emit`; read metadata from the built
`.wasm`."** *Refuted* by the WASM lens: `agent_info()` is `#[cfg(not(target_arch = "wasm32"))]`, so
the metadata does not exist in the `.wasm` to read. **[audit]** It was also mis-rated `L` when it is
the highest-*risk* item in the agent cluster — it changes the shared `runtara:agent` WIT contract.
The cheap partial win is A1, which removes the per-build relink of the widest fan-in node; re-assess
only after that.

**R3 — "Move `validation.rs`'s 7.4k inline test lines into `tests/` for a build win."** *Refuted* —
it is a net **loss**. Inline `#[cfg(test)]` code compiles only under `cargo test`/`--all-targets`;
moving it to `tests/` creates a separate crate that must link the whole library. More generally:
**splitting files never changes build time**, because the crate is cargo's unit of both parallelism
and recompilation. Only crate boundaries move that needle. This applies equally to the large agent
`lib.rs` files.

**R4 — "Split `runtara-dsl` into a stable-types crate and a churny remainder."** *Refuted by churn
data* (§1.6): the churn is in `schema_types.rs` and `agent_meta.rs`, exactly what the 27 agent
crates consume. The split would leave agents on the churny side. The viable direction is the
reverse — move `form/`, `spec/`, `condition_eval` **out**. Note also that `spec` and
`step_registration` are already `json-schema`-gated, so some fan-out claims about them are invalid.

**R5 — "Remove `runtara-validation-wasm` from `[workspace.members]`"** (to stop it forcing `wasm-js`
onto peers). *Refuted / eliminated* — it becomes unnecessary the moment **B1** lands, and removing
it from members would give it its own lockfile and reintroduce independent-toolchain skew. **[audit]**

**R6 — My own initial suspicions, corrected by checking dependency *kind*.**
`component-host → agent-crypto` and `environment → workflows` are **dev-dependencies**, so the
environment binary does **not** link the wasm compiler toolchain, and the host does not depend on a
specific agent in the shipped graph. `report-dsl → object-store` is optional behind a documented
WASM-safety feature. All three are healthy. A flat grep over manifests that ignores
`[dev-dependencies]`/`[build-dependencies]` produces a misleading graph — the corrected table is in
§Appendix A.3.

---

## Part 6 — Decisions needed

These are genuine forks the audit could not settle; each blocks work in Phase 4/5.

**Decision 1 — Where does the shared agent wire vocabulary live?** Three findings proposed three
homes for `AgentError` / `FileData` / `RawConnection` (currently duplicated in 20–23 agent crates):
a new leaf `runtara-agent-prelude`; `runtara-dsl` (already a dep of all 27, and lean with
`default-features = false`); or a third new shared crate. **Must be resolved before E1 starts** —
otherwise the same types move twice. Note the tension: putting them in `runtara-dsl` grows the crate
whose 27-agent fan-out other proposals want to shrink.

**Decision 2 — What is the canonical source of truth for step types?** One finding adds
`Step::type_name()` to `runtara-dsl`; another defines `DirectStepType` with 14 serde renames inside
the new manifest-contract crate, which it wants to depend on `serde` only. Both cannot be canonical.
**Blocks E4.**

**Decision 3 — There are two half-built routes shipping Rust to the frontend, and no finding retires
either.** `runtara-validation-wasm` is *built* (by `build.rs` + `build-validation-wasm.mjs`);
`runtara-report-dsl`'s bundle is *hand-vendored and 6 commits stale*. Pick one mechanism.
**Blocks D11 and the browser half of E5.**

**Decision 4 — Is `crates/runtara-server/frontend` the frontend's home?** A 140k-LOC TS app with its
own CI, dead-code config and e2e suite living inside a Rust crate. This is a product/ownership call,
not a technical one; F1 and D3 both touch it.

**Decision 5 — Does anything need a statically-linked binary?** If no, A2 can drop `vendored`
outright instead of hiding it behind a feature. Everything found points to no: `scripts/build-bundle.sh:77-78`
builds only `${ARCH}-unknown-linux-gnu` and `${ARCH}-apple-darwin`; `docker/Dockerfile:20` is
`ubuntu:22.04` and documents "the dynamically-linked runtara-server binary"; and a grep for `musl`
across every `*.yml`/`*.sh`/`*.toml`/`Dockerfile*` finds only two explanatory comments plus an
unused `[target.x86_64-unknown-linux-musl]` linker section in `.cargo/config.toml:4`. **[verified]**

(An earlier draft cited `CHANGELOG.md:198` as saying the musl path was removed. It does not — it says
the native-musl path "is retained as a fallback and flagged for cleanup", and it is about
`RUNTARA_COMPILE_TARGET` for *scenario compilation*, not about how the server binary links. That
citation is withdrawn; the evidence above is independent of it and stronger.)

---

## Appendix — Reproducing the measurements

Use a scratch target dir so `./target` is untouched.

```bash
export CARGO_TARGET_DIR=/tmp/runtara-timing
cargo check --workspace --all-targets --timings
# → target dir's cargo-timings/cargo-timing.html; UNIT_DATA in the HTML has per-unit start/duration
```

**A.1 — the no-op floor and its cause**
```bash
cargo check --workspace --all-targets   # run twice; second should be a no-op and is not
CARGO_LOG=cargo::core::compiler::fingerprint=trace cargo check -p runtara-agent-xml 2>&1 | grep -i stale
```

**A.2 — the OpenSSL share of the critical path**
```bash
OPENSSL_NO_VENDOR=1 PKG_CONFIG_PATH="$(brew --prefix openssl@3)/lib/pkgconfig" \
  cargo check --workspace --all-targets --timings
# compare the openssl-sys / libssh2-sys run-custom-build units between the two reports
```

**A.3 — dependency edges by kind** (the flat-grep trap in R6): parse each manifest's
`[dependencies]` / `[dev-dependencies]` / `[build-dependencies]` sections separately and record
`optional` / `default-features` / `features` per edge. Normal first-party edges only:

```
component-host:  agent-wit, dsl, workflow-wit
connections:     dsl, agents[no-default]
environment:     core, component-host, dsl
report-dsl:      dsl[no-default,utoipa], object-store[OPTIONAL]
sdk:             core[OPTIONAL], http[OPTIONAL], sdk-macros
server:          workflows, management-sdk, dsl[utoipa], agents[no-default,native], core[server],
                 environment, object-store, text-parser, connections[utoipa], ai,
                 report-dsl[utoipa], component-host
validation-wasm: dsl, workflows[no-default,wasm-js]
workflow-stdlib: sdk[OPT,no-default,http], ai[OPT,no-default], http[OPT,no-default]
workflows:       agents[no-default]  ← DEAD (B1), dsl, workflow-wit[OPTIONAL]
agents/*:        agent-macro, dsl[no-default] (+ http target-gated native/wasi, + ai, + encoding)
dev-deps:        component-host→agent-crypto · environment→workflows · sdk→core
                 workflows→workflow-stdlib,component-host · agent-macro→dsl · agent-http→http
build-deps:      server→dsl
```

**A.4 — churn**
```bash
git log --oneline --since='90 days ago' -- crates/<name> | wc -l
```

**A.5 — coupling inside a crate** (extraction seams)
```bash
grep -rhoE 'crate::[a-z_]+(::[a-z_]+)?' src/<subsystem>/ | sort | uniq -c | sort -rn
```

**A.6 — feature unification reality check**
```bash
# Unique-package counts for the B1 dead edge. --prefix none + dedup is required:
# a bare `wc -l` counts tree rows, not distinct packages.
count() { cargo tree -p runtara-workflows -e normal,build --prefix none "$@" \
            | awk '{print $1" "$2}' | sort -u | wc -l; }
count                                          # 194  (default = compiler + native-agents)
count --no-default-features --features compiler # 131

# The native C tail that the dead edge drags in (prints 6 today, 0 after B1):
cargo tree -p runtara-workflows -e normal,build --prefix none \
  | grep -cE '^(openssl-src|ssh2|libssh2-sys|calamine|zip) '

cargo tree -p runtara-server -e features -i sqlx-core   # both TLS backends
cargo tree --duplicates --workspace
```
