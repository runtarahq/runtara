# SQLite Removal Plan

**Repo:** `/Users/volodymyrrudyi/work/runtara` @ `main` (clean, `ec2fde6e`). All line numbers verified against the tree on 2026-08-28.

> Produced by a six-dimension survey (core persistence, test consumers, deps/build, docs/scripts/CI,
> downstream consumers, schema semantics), each adversarially verified, plus a completeness pass.
> Where surveys disagreed, the corrected reading is used and the disagreement noted inline.
> Independently re-confirmed before commit: the `migrations/postgresql/{013,016,017}` checksum hazard;
> `dialect/mod.rs:{13,17,20}`; `migrations.rs` ending at line 40; `common/error.rs:70-74`;
> `scripts/build-bundle.sh:216` shipping only `runtara-server`; the `if/else` shape of
> `connect_persistence`; and the un-gated `parity_harness.rs:{362,372}` imports.

---

## Why

`SqlitePersistence` has exactly one non-test consumer (`crates/runtara-core/src/main.rs:138-151`, a binary that is never shipped — `scripts/build-bundle.sh:216` copies only `runtara-server`), while the SQLite schema stopped tracking Postgres nine migrations ago (`migrations/postgresql/` reaches 017; `migrations/sqlite/` stops at 011, and 009 is a literal `SELECT 1;` no-op). Its divergences have leaked into production code as defensive guards (`runtara-environment/src/wake_scheduler.rs:167-170`, `src/runner/embedded.rs:288-293`) and have switched off four real assertions in `runtime_host.rs`. The sqlx `sqlite` feature at `crates/runtara-core/Cargo.toml:33` is the workspace's only one, and it drags `sqlx-sqlite` + `libsqlite3-sys 0.30.1` (an 8.7 MiB vendored C amalgamation, ~13 s serial compile) into every build of every crate.

---

## Scope

**In scope (this plan):**
- Delete `persistence/sqlite.rs`, `persistence/dialect/sqlite.rs`, `migrations/sqlite/`, `migrations::{SQLITE, run_sqlite}`, `RowsAffected for SqliteQueryResult`, the module wiring, and the sqlx `sqlite` feature.
- Make `connect_persistence` reject non-Postgres URLs loudly.
- Port the 32 orphan core persistence tests to the Postgres suite **before** deleting them.
- Re-host the 32 environment lib tests + 5 `embedded_runner_test.rs` tests.
- Convert the two core e2e scripts to Postgres.
- Delete every dangling intra-doc link and rewrite SQLite prose in surviving files.
- CHANGELOG `### Removed`, README, crate rustdoc, config docs.

**Explicitly NOT in scope — follow-up phases, not this one:**
- The `Dialect` trait collapse (`dialect/mod.rs` + `dialect/postgres.rs`, 602 lines) and the `common/ops/` macro expansion (~1,078 lines of `macro_rules!`). Both compile unchanged with one backend.
- `SKIP LOCKED` / atomic wake-claim rewrite. Requires compensating re-stamps on three early-return paths in `wake_scheduler.rs` — a correctness change, not a removal.
- Any new `018+` migration (BYTEA→JSONB, dropping `pending_checkpoint_signals.id`). **The removal touches zero files under `migrations/postgresql/` and adds none.** This is what keeps rollback one redeploy.
- `not_found_if_empty` de-genericization, `decode_json_text` removal, transaction/isolation work.
- Pre-existing defects surfaced but unrelated: `crates/runtara-core/README.md:5-6` crates.io/docs.rs badges and `:17` `runtara-core = "4.0"` pin (workspace is 8.7.8); `.claude/skills/release/SKILL.md:3,49` claims a crates.io publish step that does not exist.

**Hard boundary — do not cross:** `crates/runtara-core/migrations/postgresql/*.sql` is checksum-validated by `sqlx::migrate!` at every boot. Files `013:11`, `016:6` and `017:17` contain SQLite prose in **already-applied** migration headers. Editing any byte of them makes every existing production database fail startup with `MigrateError::VersionMismatch`. Any grep/sed sweep MUST exclude that directory.

---

## Phases

### Phase 0 — Convert the two core e2e scripts to Postgres (highest risk, lands first)

**Goal:** Prove the standalone core binary works on Postgres *while SQLite still exists*, so a later failure is unambiguously attributable.

| File | Change |
|---|---|
| `e2e/test_core_sigterm_drain.sh` | `:21` header (drop "needs cargo, curl and SQLite only. No docker, no Postgres"); after `:34` add `POSTGRES_{HOST,PORT,USER,PASSWORD}` defaults + `TEST_DB="runtara_e2e_core_sigterm_$$"` + `psql_quiet()` (copy `e2e/test_connection_named_endpoint.sh:54-57,87-92`); pre-flight `SELECT 1` **before** the `cargo build` at `:72`; `DROP…WITH (FORCE)` + `CREATE DATABASE` + `CREATE EXTENSION IF NOT EXISTS pgcrypto` before `:92`; `DROP DATABASE … WITH (FORCE)` in `cleanup()` at `:44-50`; `:93` print_step text; `:99` URL; `:110` readiness loop `seq 1 100` → `seq 1 200` |
| `e2e/test_core_http_status_codes.sh` | Same six edits at `:20`, after `:32`, before `:66`, `:38-44`, `:85`, `:91`, `:100`. DB name `runtara_e2e_core_status_$$` |
| `e2e/run_all.sh` | `:84-92` — collapse the two comment blocks; delete "the cheapest test here" and "no docker, no Postgres" |

**Non-obvious constraints:**
- **`op_count_active_instances` (`common/ops/instances.rs:389-396`) is `SELECT COUNT(*) FROM instances WHERE status='running'` with no tenant filter — database-global.** The SIGTERM cap assertion (`:137-149`) therefore requires an empty `instances` table. A per-run throwaway DB is mandatory, not hygiene; `$$` suffix prevents concurrent-run collision.
- `WITH (FORCE)` (PG13+; dev is pg18, CI pg16, `e2e-verify` skill already documents "Postgres 14+") is required, not cosmetic: every `fail()` path `kill -KILL`s the core, and a plain `DROP` loses to "database is being accessed by other users", leaking one DB per aborted run.
- `pgcrypto`: `migrations/postgresql/` contains no `CREATE EXTENSION` and uses `gen_random_uuid()` only (`001:121`, PG13+ builtin), but `parity_harness.rs:417-420,444-447` provisions pgcrypto on both branches before running the *same* migrator. Mirror it — one idempotent line — rather than leaving the e2e scripts and the parity harness disagreeing. *(The docs-scripts survey claimed it unnecessary; the schema-semantics verifier is right that the repo's own working path disagrees.)*
- **Keep the `cd "${WORK_DIR}"` subshell**, but write the true reason. `main.rs:28` is `dotenvy::dotenv().ok()` — the **non-overriding** form, and both scripts set `RUNTARA_DATABASE_URL` explicitly, so `.env:8` can never reach the binary. *(Three of six surveys claimed a data-loss risk here; it is false.)* What the guard actually blocks is `.env:40-41` (`OTEL_SDK_DISABLED=false`, OTLP endpoint) and `.env:44` `RUST_LOG=info`, which would override the scripts' `runtara_core=info` filter that the log-ordering assertions at `:207-230` and the WARN-not-ERROR assertions at `test_core_http_status_codes.sh:196-201` depend on.
- Naming the DBs `runtara_e2e_core_*` (not `core_*_e2e_*`) folds them into `.claude/skills/reset-local-env/SKILL.md:18`'s `runtara_e2e%` LIKE sweep, so a leaked DB is visible.

**Verify:** `bash e2e/test_core_sigterm_drain.sh && bash e2e/test_core_http_status_codes.sh` — both green on the unmodified tree.

**If this lands alone:** nothing breaks. No workflow runs `e2e/*.sh` (the only `e2e` references in CI are the frontend Playwright suites at `frontend.yml:235,282-283` and `release.yml:241`), so these are local-and-manual — which is exactly why they must be proven by hand here rather than trusted to CI later.

**Owner decision:** these two scripts stop being the suite's only infrastructure-free entries. Accept, or move them under an opt-in flag in `run_all.sh`.

---

### Phase 1 — Port the 32 orphan core persistence tests (additive, always green)

**Goal:** `postgres.rs`'s gated test module covers everything `sqlite.rs`'s 46 tests covered before any deletion.

`crates/runtara-core/src/persistence/postgres.rs:763-1304` gains the 32 tests with no Postgres twin. Two families carry the actual product risk:

- **All 11 step-summary tests** (`sqlite.rs:1623-2346`). The Postgres step-summary CTE (`dialect/postgres.rs:192-363`, with its `MATERIALIZED` + `OFFSET 0` planner fences) has **zero** test coverage today — `parity_harness.rs:189-198` only asserts it returns empty. This CTE powers `/steps`, the timeline and Graph Replay and is a known perf hazard. Port these first. Requires porting the `insert_step_start`/`insert_step_end` helpers from `sqlite.rs:1549-1620`, rewriting their raw `?` placeholders to `$N`.
- **All 8 unified `complete_instance` tests** (`sqlite.rs:1171-1458`) — the `if_running` guard, `InstanceNotFound`-vs-`Ok(false)`, the non-terminal `finished_at` CASE, and the relaunch clearing of stale `finished_at` (a real past bug: negative durations on resume). *(Both the survey and its notes said "7"; there are eight.)*

Plus: `get_instance_not_found`, `list_checkpoints` (+filter, +count), `signal_upsert`, `custom_signal_upsert`, `save_retry_attempt`, `list_instances`, `list_instances_by_status`, `complete_instance_extended`, `update_instance_metrics`, `update_instance_stderr`, `store_instance_input`.

**Porting rules (get these wrong and the port is silently worthless):**
1. **Construct `PostgresPersistence::new(pool)` and call the `Persistence` trait** — do *not* copy `postgres.rs`'s existing static `PostgresPersistence::op_*(&pool, …)` style. `insert_signal` (`postgres.rs:389`) and `insert_custom_signal` (`:422`) are free functions; `update_instance_metrics`/`update_instance_stderr` are inline trait impls (`:715-738`). The closest precedent is `parity_harness.rs:396-399`.
2. **Tenant-scope or delta-ify every global assertion.** CI runs `--test-threads=1` against ONE shared `runtara_test` DB (`ci.yml:290-291`), and rows persist across tests. Three tests need this, not two: `test_list_instances`, `test_count_active_instances` (`postgres.rs:1241` already shows the before/after delta pattern), **and `test_list_instances_by_status`** (`sqlite.rs:1047-1076`, asserts `running.len() == 1` with a `None` tenant filter).
3. Use a fresh `Uuid::new_v4()` tenant + instance id per test; clean up.
4. Rewrite the four raw-SQL verifications from `?` to `$N`: `sqlite.rs:1476-1478`, `:1505-1507`, `:1533-1535`, and the step helpers.

**Semantic ports, not copies:**
- `test_acknowledge_signal` (`sqlite.rs:891-919`) asserts `acknowledged_at.is_some()` — it encodes the bug. `postgres.rs:1177` already has the correct `is_none()` version. **Delete, do not port.**
- `test_update_instance_metrics`/`_stderr` (`sqlite.rs:1460-1514`) must flip to Postgres COALESCE first-writer-wins and gain a second-write assertion.
- `test_signal_upsert` (`sqlite.rs:861-889`): **port assertions unchanged.** The `b""` goes into the first insert, which is immediately upserted away by `insert_signal(…, "cancel", b"new reason")`; the surviving payload is `b"new reason"` on both backends. *(The survey told you to change this to `payload == None`, which would convert a passing assertion into a failing one.)* If you want the empty-payload→NULL divergence (`common/ops/signals.rs:9-13`) covered, add a **new** test that inserts `b""` and never overwrites.
- `test_save_retry_attempt` can be **strengthened** on Postgres: assert the dedicated `is_retry_attempt`/`attempt_number`/`error_message` columns (`postgres.rs:302-316`), not just the `::retry::1` checkpoint's existence.

**Verify:**
```
TEST_RUNTARA_DATABASE_URL=postgres://... \
  cargo test -p runtara-core --features db-integration-tests -- --test-threads=1
```
**If this lands alone:** nothing breaks — purely additive.

---

### Phase 2 — Re-host the 37 runtara-environment tests

**Goal:** `runtara-environment` has no `SqlitePersistence` reference before the atom lands.

Affected: `src/runtime_host.rs` (21 tests, one fixture `setup()` at `:585-616`), `src/runner/embedded.rs` (8 tests, fixtures `running_instance()` `:815-832` and `backstop_fixture()` `:984-1000`), `src/http_server.rs` (3 waker tests, fixture `suspended_instance()` `:2146-2172`), `tests/embedded_runner_test.rs` (5 tests, `harness()` `:67-87`, and `struct Harness { persistence: Arc<SqlitePersistence> }` at `:63`).

**This is the one genuine owner decision in the plan.** Two routes:

**(a) Postgres + `db-integration-tests` gate** — smaller diff, matches every other DB-bound test here. Gate the three `src/` modules with `#[cfg(all(test, feature = "db-integration-tests"))]`; add a `[[test]]` block with `required-features = ["db-integration-tests"]` for `embedded_runner_test.rs` to `crates/runtara-environment/Cargo.toml` (it and `instance_output_test.rs` are the only two auto-discovered, ungated test targets). Cost: 32 tests leave the free `cargo test --workspace --lib` gate. Read `TEST_ENVIRONMENT_DATABASE_URL` (what `ci.yml:394` sets), **never** `RUNTARA_DATABASE_URL` — `main.rs:28`'s `dotenvy` + `.env:8` would silently point a dev machine's fixture at the live `localhost:5432/runtara`. Fixture pattern to copy: `tests/db_test.rs:18-36`, or `parity_harness.rs:410-453` for env-else-testcontainers.

**(b) In-memory mock** — keeps all 32 in the free gate; these tests assert host orchestration (rate limiter, escalation, cancel latch, sleep stamping), not SQL. Prerequisites the surveys understated:
- `crates/runtara-core/src/lib.rs:153` is `#![deny(missing_docs)]`. Promoting `instance_handlers/mock_persistence.rs` from `#[cfg(test)] pub(crate)` (`instance_handlers/mod.rs:26-27`) to a `pub` `test-support` module requires documenting 10+ newly-public items, and its module doc's "zero cost in release builds" claim becomes false (a dev-dep feature unifies it into the normal lib build).
- The mock implements 26 of the trait's 36 `async fn`. Ten fall through to trait defaults: `claim_sleeping_instance`, `delete_instances_batch`, `get_last_error`, `get_terminal_instances_older_than`, `list_errors`, `mark_for_recovery`, `record_error`, `store_instance_input`, `update_instance_metrics`, `update_instance_stderr`. Three block these specific tests and must be implemented: `store_instance_input`, `insert_custom_signal` (`:332-339`, currently a no-op), `list_events`/`count_events` (`:406-422`, return empty).
- **Do NOT "fix" the mock's signal handling.** `acknowledge_signal` (`:327-330`) removes the row, so `get_pending_signal` (`:320-325`) returns `None` — that is already Postgres semantics. *(One survey's ordering constraint asserted the opposite; verified false at the source.)*

**Recommendation: (b) for the three `src/` modules, (a) for `tests/embedded_runner_test.rs`** — the latter's assertions are about wasmtime run/launch/stop mapping and touch persistence only via `get_instance`/`load_output`/`complete_instance`/`set_instance_sleep`, so either works, but its module doc (`:6`) already advertises hermeticity, which (b) preserves. Whichever you pick, add a standing rule: **no behavior may be tested against the mock alone** — a mock drifting from Postgres is precisely the failure class SQLite divergence caused.

**Under either route, fixed instance ids become collisions.** `op_register_instance` (`common/ops/instances.rs:43-55`) is a bare `INSERT` with no `ON CONFLICT`; `"waker-inst"` ×3, `"park-inst"` ×5, `"inst-1"` ×3, `"rt-host-inst"` ×21, `"inst-ok"` et al ×5 each shared nothing under SQLite's per-test temp file. `--test-threads=1` does not help — sequential tests share one database. Replace `const INSTANCE` with `fn instance_id() -> String` before porting, not after.

**Restore the four weakened assertions in the same commit** (see the test table below).

**Verify:** `cargo test -p runtara-environment --lib` (route b) or `TEST_ENVIRONMENT_DATABASE_URL=… cargo test -p runtara-environment --features db-integration-tests -- --test-threads=1` (route a).

**If this lands alone:** nothing breaks. If it lands *after* Phase 3, the workspace does not build in between.

---

### Phase 3 — THE ATOM (single commit; there is no intermediate green state)

**Goal:** SQLite is gone from code, deps and the lockfile.

Delete:
- `crates/runtara-core/src/persistence/sqlite.rs` (whole file, 1-2397)
- `crates/runtara-core/src/persistence/dialect/sqlite.rs` (whole file, 1-393)
- `crates/runtara-core/migrations/sqlite/` (11 files)
- `crates/runtara-core/src/persistence/mod.rs:8` (`pub mod sqlite;`) and `:11` (`pub use … SqlitePersistence;`)
- `crates/runtara-core/src/persistence/dialect/mod.rs:17` and `:20` — **four of six surveys missed these; deleting `dialect/sqlite.rs` without them is E0583 + E0432**
- `crates/runtara-core/src/migrations.rs:23-24` and **`:34-40`** — not 34-39; `run_sqlite`'s closing `}` is line 40, the file's last line, and cutting 34-39 leaves a stray brace
- `crates/runtara-core/src/persistence/common/error.rs:70-74` (`impl RowsAffected for sqlx::sqlite::SqliteQueryResult`) — the only un-gated, non-test sqlx-sqlite reference in the library
- `crates/runtara-core/src/persistence/common/ops/parity_harness.rs:362`, `:372`, `:374-388`; change `:357` to `#[cfg(all(test, feature = "db-integration-tests"))]` and drop the now-redundant per-item `#[cfg(feature = …)]` at `:360,363,365,367,370,394,410`
- `crates/runtara-core/src/persistence/common/ops/mod.rs:35-36` only if `parity_harness.rs` is deleted outright (**recommended: keep it**, renamed `postgres_conformance.rs` — see below)
- `crates/runtara-environment/src/cleanup_worker.rs:187-189` (the `.db`-file skip; already dead behind the `is_dir` guard at `:171-173`)

Edit:
- `crates/runtara-core/Cargo.toml:33` — drop `"sqlite"` from the sqlx feature array, leaving `["runtime-tokio","tls-rustls-ring-webpki","postgres","uuid","chrono","macros","migrate"]`. Verified the only such declaration in the workspace.
- `crates/runtara-core/src/main.rs:18` **and `:22`** (drop `SqlitePersistence` from the `runtara_core::persistence::{…}` import — a different error class the "drop the feature and let the compiler list the sites" technique will not surface), `:113-115` doc, `:119-152`. The function is `if <pg> { … } else { <sqlite> }` with both arms returning the value — **deleting the `else` leaves the `if` with no value on the false path.** Rewrite to reject any non-`postgres://`/`postgresql://` URL with an error naming `RUNTARA_DATABASE_URL` and the required scheme. Today *any* unrecognized string silently becomes a SQLite path; without an explicit rejection the operator gets a raw sqlx URL-parse error. Keep the `Arc<dyn Persistence>` return type.
- `Cargo.lock` — regenerate and commit.

Dangling intra-doc links (must ride in this commit or `cargo doc` warns immediately; no CI consequence — there is no `cargo doc` job — but they are in files you are already editing):
- `common/mod.rs:13` (`[super::sqlite::SqlitePersistence]`)
- `common/ops/mod.rs:13-17` (`[crate::persistence::sqlite]` + the stale "Phase 1 (SYN-394) … until each family is migrated")
- `common/ops/retention.rs:10-18` (`[crate::persistence::dialect::SqliteDialect::exec_delete_instances_batch]` — points into a file being deleted) and `:54-58`
- `dialect/mod.rs:12-14` (`[super::sqlite]`) — fold into the same delete-region as `:17,:20`

Operator-facing docs (ordering constraint: a README promising "PostgreSQL or SQLite" beside a binary that rejects `sqlite://` is worse than either state):
- `crates/runtara-core/README.md:3, 11, 50, 56` — `:56` ("Depends on `sqlx` (Postgres + SQLite)") becomes factually false the instant `Cargo.toml:33` changes
- `crates/runtara-core/src/lib.rs:34` (keep the ASCII box width) and `:133`
- `crates/runtara-core/src/config.rs:18, 32, 240, 246` — `:240/:246` set and assert `"sqlite:test.db"`. This test needs no database, so it will keep **passing** after the removal and silently preserve a `sqlite://` example. Nothing will ever flag it.
- `crates/runtara-sdk/src/client.rs:49`
- `CHANGELOG.md` — insert `### Removed` **between `### Changed` (ends 124) and `### Fixed` (125)**, per the Keep-a-Changelog 1.1.0 order the file declares at `:5`. This is a genuine operator break: a `sqlite://` `RUNTARA_DATABASE_URL` works today for the standalone `runtara-core` binary and will now fail at startup.

**Ordering inside the commit:** author the source deletions first, drop `Cargo.toml:33` **last** — while `"sqlite"` is present, `cargo check` cannot enumerate the remaining sqlx-sqlite type references; removing it turns the compiler into an exhaustive checklist.

**Two migrate! sites, not one.** `sqlx::migrate!("./migrations/sqlite")` appears at `migrations.rs:24` **and** `persistence/sqlite.rs:16` (`static MIGRATOR`). Both resolve the directory at macro-expansion time. Four of six surveys named only the first.

**Clean-build-only failure mode.** `runtara-core` has no `build.rs`, and `sqlx::migrate!`'s `proc_macro::tracked_path` call (`sqlx-macros-core-0.8.6/src/migrate.rs:119`) is behind `#[cfg(any(sqlx_macros_unstable, procmacro2_semver_exempt))]` — neither is set (stable 1.97.0 pinned in `rust-toolchain.toml`; `.cargo/config.toml` sets only lld link-args). So deleting `migrations/sqlite/` while either migrate! site survives can **pass an incremental `cargo check` from cache** and fail only on a fresh checkout. Run `cargo clean -p runtara-core` before verifying this commit.

**Cargo.lock — the one place two surveys disagreed.** deps-build *measured* a one-line diff (`- "cc",` at `Cargo.lock:2539`, from `sqlite → sqlx-sqlite/bundled → libsqlite3-sys/bundled → cc`); its verifier *reasoned* from `sqlx-mysql` that nothing would change. Settle it by running, not reasoning. **The correct verification is a graph check, never a lockfile grep:** `grep -c 'libsqlite3-sys\|sqlx-sqlite' Cargo.lock` will still return 5 afterward and that is **correct** — Cargo.lock is feature-independent and keeps `[[package]]` entries for optional deps no feature activates. Also note `scripts/release.sh:68` runs `cargo update --workspace` three lines before the `--locked` check at `:71`, so a stale lockfile is silently absorbed rather than breaking the release preflight (contra one survey's `breaksBuildIfMissed: true`).

**Verify:**
```
cargo clean -p runtara-core
cargo tree -p runtara-core -e normal -i libsqlite3-sys     # must print "nothing to print"
cargo tree -p runtara-core -e normal -i sqlx-sqlite         # must print "nothing to print"
git grep -in sqlite -- crates/ ':!crates/runtara-core/migrations/postgresql'   # only intentional survivors
cargo build --workspace --all-targets
```

**If any piece lands alone:** the workspace does not compile. `common/error.rs:70-74`, `dialect/sqlite.rs:21,42`, `persistence/sqlite.rs:6-7,16`, `migrations.rs:38`, `main.rs:18,22`, `parity_harness.rs:362,372` all name types that vanish with the feature.

---

### Phase 4 — Prose sweep: the 19 stale `SYN-394` comments

**Goal:** stop 15 files narrating a two-backend unification project that ends with Phase 3, referencing an external plan doc that does not exist in `docs/`.

`crates/runtara-core/src/persistence/`: `postgres.rs:136`, `common/filters.rs:7`, `common/error.rs:3-15`, `common/mod.rs:15`, `common/row.rs:9,12,39`, `dialect/mod.rs` (module doc + the 15 per-method Postgres-vs-SQLite comparison docs at `:6,13,23,49,54,69,73,81,85,93,99,108,116,139,154,163,180`), `dialect/postgres.rs:15-21`, `common/ops/{mod.rs:13-14, checkpoints.rs:8-26, events.rs:8-17, instances.rs:11-26, retention.rs:12-18,57, signals.rs:10-19,30, sleep.rs:10-14, step_summaries.rs:13,16}`.

Plus the two production comments whose premise disappears — **keep the guards, rewrite only the prose**: `runtara-environment/src/wake_scheduler.rs:167-169` (guard at `:170`) and `src/runner/embedded.rs:288-291` (guard at `:293`). Both are correct on Postgres, free, and removing them makes correctness depend on backend SQL forever. And `instance_handlers/checkpoint.rs:136-141` / `:480-486` — retarget the rationale at Postgres (23503 lands in the identical `CheckpointSaveFailed`/Transient path), do not delete: they are the only record of why `ensure_instance_running` exists.

Resolve each documented divergence by **adopting the Postgres side**, never merging: checkpoint save = upsert; `insert_event` = honour the caller's `created_at`; `insert_signal`/`insert_custom_signal` = empty payload → NULL; metrics/stderr = COALESCE first-writer-wins; `get_pending_signal` = filter `acknowledged_at IS NULL`; `payload_contains` = case-**insensitive** (`ILIKE`) — and fix `persistence/mod.rs:148-149`, which two other files claim documents this divergence but does not.

Also: rename `parity_harness.rs` → `postgres_conformance.rs`. A "parity harness" with one backend is a misnomer, but `run_parity_sequence` (`:25-355`) is the only *unit-level, core-crate* coverage for `set_instance_sleep`, `get_sleeping_instances_due`, `claim_sleeping_instance` double-launch prevention, `clear_instance_sleep`, `get_terminal_instances_older_than` and `delete_instances_batch`. *(The survey called it the only coverage anywhere — false: `runtara-environment/tests/wake_scheduler_test.rs:526-604` and `tests/db_cleanup_worker_test.rs` exercise all of it end-to-end on Postgres. Keep it for the unit-level contract, not because deleting it is a cliff.)* Two assertions in it were softened by parity and should harden now: `:160-163` acknowledges a signal and asserts nothing; `:189-198` asserts step summaries are merely empty. **Leave `:177-180` intact** — it documents the non-destructive `take_pending_custom_signal` read, a backend-independent durability invariant, not parity prose.

**Verify:** `cargo doc -p runtara-core --no-deps 2>&1 | grep -i warn` — clean. `git grep -c SYN-394 -- crates/` → 0.

**If this lands alone:** nothing breaks. Comments only.

---

## Test migration table

91 SQLite-dependent tests. Rows are per-test where the port is semantic, per-family where it is mechanical — listing 46 identical "port verbatim" rows would bury the eight that matter.

| Current location | Asserts | Replacement | Strengthenable? |
|---|---|---|---|
| `core/persistence/sqlite.rs:1623-2346` (11 step-summary) | empty/completed/running/failed step, launch-settle, output-error envelope, filter by status / step_type / step_ids (incl. `step-"quoted"` injection guard), pagination, scopes | Port → `postgres.rs` gated module + `insert_step_*` helpers, `?`→`$N` | **Yes — this is the only coverage the shipping Postgres CTE will ever have** |
| `sqlite.rs:1171-1458` (8 unified `complete_instance`) | if_running success/skipped, unguarded miss→`InstanceNotFound`, unguarded success→true, guarded miss→false, non-terminal preserves `finished_at`, running clears `finished_at`, termination COALESCE | Port verbatim, PG shape | No |
| `sqlite.rs` — 13 misc orphans (`get_instance_not_found`, `list/count_checkpoints`(+filter), `signal_upsert`, `custom_signal_upsert`, `save_retry_attempt`, `list_instances`, `complete_instance_extended`, `store_instance_input`) | as named | Port; construct `PostgresPersistence::new(pool)` + trait calls | `save_retry_attempt` → assert `is_retry_attempt`/`attempt_number`/`error_message` columns |
| `sqlite.rs:1047-1076` `test_list_instances_by_status` | `list_instances(None,"running",…).len()==1` | Port **tenant-scoped** — whole-DB assertion breaks on shared CI DB | No |
| `sqlite.rs:1460-1514` metrics/stderr | last-writer-wins | Port, flip to COALESCE first-writer-wins + second-write assertion | Yes — pins the surviving semantics |
| `sqlite.rs:891-919` `test_acknowledge_signal` | `acknowledged_at.is_some()` — encodes the bug | **Delete.** `postgres.rs:1177` has the correct `is_none()` version | n/a |
| `sqlite.rs:861-889` `test_signal_upsert` | upserted payload `== b"new reason"` | Port **unchanged** — the `b""` is overwritten; PG agrees | Optional new test for empty→NULL |
| `sqlite.rs` — remaining 14 with PG twins | register/get, status, checkpoint, complete ×2, save+load ckpt, load-not-found, insert event, insert+get signal, pending-none, ack, custom signal, count active, health | **Delete** — twins exist | n/a |
| `dialect/sqlite.rs:350-393` (6) | `?N` placeholders, empty enum_cast, json_extract, IN fan-out, julianday | **Delete** — `dialect/postgres.rs:366-410` (5) survive unchanged | n/a |
| `parity_harness.rs:374-388` | full 30-op sequence on `sqlite::memory:` | **Delete driver, keep `run_parity_sequence`**; gate module on `db-integration-tests` | `:160-163` (asserts nothing after ack), `:189-198` (asserts only empty) |
| `core/config.rs:235-249` | `"sqlite:test.db"` round-trips | Swap both literals to `postgres://`. **Will keep passing if missed** | No |
| `env/runtime_host.rs:619-1102` (21, fixture `:585-616`) | load_input, checkpoint miss/hit, custom-signal re-read, complete/fail, events, cancel/pause/shutdown, durable sleep, retry audit, 8-test SYN-606 escalation suite, rate limiter | Mock (recommended) or Postgres; unique instance id | **4 restorations — see next 4 rows** |
| ↳ `:731-749` `cancel_signal_is_consumed_acked_and_latched` | comment `:739-742` drops the pending-row assertion "SQLite returns acknowledged rows" | add `assert!(get_pending_signal(…).is_none())` | **Yes** |
| ↳ `:751-763` `pause_signal_suspends_via_check_signals` | same omission, `:758-759` | same | **Yes** |
| ↳ `:765-780` `shutdown_signal_suspends_with_reason_and_wake` | `:772-777` says `termination_reason` + `sleep_until` "asserted on Postgres only" — CHECK frozen at sqlite/008. **Comment is stale:** `sqlite/011_rebuild_termination_check.sql` fixed it and the assertion was never restored | assert `termination_reason == Some("shutdown_requested")` **and** `sleep_until.is_some()` | **Yes — if it fails, that is a real bug this masked** |
| ↳ `:1022-1065` `fresh_artifact_polls_and_the_host_does_not_also_escalate` | `:1048-1056` `.expect("sqlite keeps the row after acknowledgement")` + `acknowledged_at.is_some()` | **Not a one-line inversion.** Bare `is_none()` cannot distinguish "acked" from "never inserted". Keep the `status=="cancelled"` assertion at `:1035-1038` as positive evidence, then assert `get_pending_signal` is `None` | **Yes** |
| ↳ `:1089-1101` rate-limiter test | `get_pending_signal(…).is_some()` | **Passes unchanged on Postgres** — the cancel is never acked (poll suppressed by the 60 s limiter), so `acknowledged_at IS NULL` still matches. Verify, don't "fix" | No |
| ↳ `:830-836` `record_retry_attempt_writes_audit_row` | nothing ("no readers to assert against"); mock's `save_retry_attempt` is a no-op | Give it a real read-back or drop it. **Do not count as preserved coverage** | Yes |
| `env/runner/embedded.rs:834-981` (5 park tests, fixtures `:815-832`, `:984-1000`) | timed wake stamps suspended+sleep_until, deadline-less OnResume untouched, OnSignal → `WAITING_SIGNAL_TERMINATION`, terminal status survives late suspend, on-signal timeout fallback | Mock handles `complete_instance` COALESCE + `set_instance_sleep`; unique ids | No |
| `env/runner/embedded.rs:1007-1090` (3 `enforce_unacked_cancel`) incl. `:1035-1069` `an_acknowledged_cancel_does_not_re_cancel_a_finished_run` | "the regression the SQLite dialect invites" | **Re-pointing degrades it to a tautology** — PG's `get_pending_signal` already filters acked rows. Either delete the guard at `:293` + this test together, or keep both and rewrite the test to assert the SQL filter directly (insert, ack, assert `None`) | No — this one gets *weaker*; be honest about it |
| `env/http_server.rs:2176-2229` (3 waker) | waker ignores pause-shaped suspend / stamps sleep for on-signal park / ignores timed sleep park | Mock models `complete_instance` + `set_instance_sleep` correctly. The other 9 tests in the module are pure and untouched | No (no weakened comments in this file) |
| `env/tests/embedded_runner_test.rs:110-222` (5) | run→output, run→error, launch_detached + registry clear, stop cancels spinning guest, missing component → `BinaryNotFound` | Persistence is incidental. `Arc<SqlitePersistence>` at `:63` is concrete → mechanical swap. Hard-coded ids (`"inst-ok"`, `:137`, `:162`, `:185`, `:213`) must become unique — these are `multi_thread` with no `#[serial]`, so they race each other | No |

---

## Follow-up simplifications unlocked

Explicitly out of scope. Each is independently sequenced; do not interleave with the removal, or a regression becomes unbisectable.

| Item | Size | Notes |
|---|---|---|
| **Collapse the `Dialect` trait** — inline `PostgresDialect`'s whole-SQL bodies, delete `dialect/mod.rs` (191) + the trait scaffolding in `dialect/postgres.rs` (411) | ~600 lines simplified | Every method has one impl: `placeholder()` is always `$N`, `normalize_timestamp()` is the identity, `in_list()` is always `= ANY($n)` (making `count` dead), `select_*_col()` are constants. Only **two** of four `EnumKind` variants need their cast suffixes inlined (`InstanceStatus`, `TerminationReason`) — `SignalType`/`InstanceEventType` are already literal in the hand-written PG inserts (`postgres.rs:404,362`). Do this **before** the ops expansion or the macros reference a deleted trait. |
| **Expand `common/ops/` macros** into plain `impl PostgresPersistence` blocks with literal `$1..$8` SQL | ~1,078 lines of macro bodies + 7 invocation sites at `postgres.rs:144-177` | Kills `format!`-assembled SQL, `::core::result::Result` hygiene armour, and restores rustfmt/IDE support. `retention.rs` collapses hardest — `op_delete_instances_batch:59-64` delegates to a per-dialect inherent purely to hide `= ANY($1)` vs a fanned-out `IN`. Watch `postgres.rs:7`'s `#![allow(dead_code)]`, which will mask newly-orphaned helpers. |
| **`not_found_if_empty` / `RowsAffected`** de-genericization | ~40 lines | Becomes `fn(&PgQueryResult, &str)`; turbofishes drop at `sleep.rs:51,70` and `instances.rs:146,168,246`. |
| **`decode_json_text` removal** | ~30 lines + 4 SQL sites | `common/row.rs:33-44` + the `::text` casts feeding it were added *for parity* and are a pure Postgres regression (serialize jsonb → re-parse in Rust, per row per column). `inputs` (`dialect/postgres.rs:291`) and `outputs` (`:294`) are clean removals; **`error` (`:236`) is not** — it is compared as text inside the same query at `:258` and `:332`, so the status CASE arms must change in the same edit. |
| **`SKIP LOCKED` atomic wake claim** — one `UPDATE … WHERE instance_id IN (SELECT … FOR UPDATE SKIP LOCKED) RETURNING …` replacing `common/ops/sleep.rs:73-141` | ~70 lines, medium risk | Real correctness win: today it is an unlocked LIMITed SELECT handing the same batch to every poller, then a racing conditional UPDATE — `sleep.rs:83-84` openly leans on "Postgres row-level locking" for a guarantee it is forbidden from writing in SQL. `idx_instances_sleep_until` already matches the predicate exactly. **Must land with compensating `set_instance_sleep(…, now())` on every early-return between claim and launch** (`wake_scheduler.rs:227-235`, `:238-242`, `:249-254`), mirroring the existing handler at `:352-366` — otherwise image-lookup failure strands the instance forever. Also: `RETURNING sleep_until` yields the post-update value (always NULL), and `status::text` needs an explicit `AS status` alias for `query_as`. |
| **Flatten `migrations/postgresql/` → `migrations/`** | 1 line (`migrations.rs:21`) | sqlx keys on version+checksum, not path, so it is safe for applied DBs. But it stales `docs/oci-native-runner-cleanup-plan.md:177` and forces a rewrite of the `.claude/skills/add-migration/SKILL.md` edit. **Decide before writing that skill edit.** |
| **`.claude/skills/add-migration/SKILL.md`** | ~15 lines | Documents only two migration dirs and never mentions `runtara-core/migrations/` — wrong since before this removal. Edit `:3` (frontmatter description — otherwise the skill is never *selected* for a core migration), `:12`, `:14-15`, `:17`, `:21-35` (core uses 3-digit sequential prefixes, next is `018_`, not 14-digit timestamps), `:87`, `:95-99`, `:101-104`. Record that core migrations used to be dual-authored so nobody hunts for the missing sibling. |
| **`instance_events.payload` BYTEA → JSONB** | Large, **conditional** | The surveys called this "the headline performance win"; it is **not straightforwardly available**. The column is deliberately polymorphic — `runtara-sdk/src/backend/embedded.rs:257` writes a bare error *string* for `failed` events and `:231` writes raw output bytes. `ALTER … USING convert_from(payload,'UTF8')::jsonb` fails on any deployment that has ever had a failed instance. Every existing `::jsonb` cast is gated behind a `subtype = 'step_debug_*'` predicate precisely because of this, and `payload_ilike` (`:58`) deliberately omits it. Any pursuit must be a **new nullable `payload_json JSONB` column** populated only for step-debug subtypes. Do not bundle with the removal — and note it is one-way (see rollback). |
| **`ErrorHistoryRecord` cleanup** (`persistence/mod.rs:237-266` + `record_error`/`get_last_error`/`list_errors` defaults) | ~60 lines | Dead residue of `postgresql/017`, **not** a SQLite artifact — SQLite never had `error_history`. Unrelated to this work. |
| **Shared test-support crate** | — | `docs/crates-structure.md:364` (finding D5) already flags "testcontainers Postgres scaffolding is copy-pasted 9 times". Phase 2 is the natural moment; also note `crates/runtara-environment/tests/common/mod.rs` (291 lines, `#![allow(dead_code)]`) is **entirely dead** — zero `TestContext` constructions and zero `common::` references across all nine files in that directory. Delete it or actually build it; do not cite it as an existing pattern. |

---

## Risks and rollback

**Rollback is one redeploy — and stays that way only if no `018` rides along.** The removal touches zero files under `migrations/postgresql/` and adds none, so reverting means redeploying the previous bundle or GHCR tag against an untouched schema. The repo has **no down-migrations anywhere** (`*.down.sql` does not exist), so the moment a follow-up `018` (BYTEA→JSONB, or dropping `pending_checkpoint_signals.id`) ships in the same release, rollback becomes one-way. **Ship them as separate releases.**

**Data migration: none exists, verified.** No released artifact can open a SQLite database. `scripts/build-bundle.sh:216` ships only `runtara-server`; `scripts/install.sh` only ever writes a `runtara-server` unit. Every default is Postgres (`install.sh:392,463`, `docker-compose.yml:82`, `e2e/install-test/docker-compose.yml:54`, `start.sh:23,187`, `README.md:147`, `CONTRIBUTING.md:90`, `.env:8,54`). The operator-visible break is narrow — a `sqlite://` `RUNTARA_DATABASE_URL` on the *unshipped* standalone `runtara-core` binary — and the CHANGELOG entry is the whole mitigation.

**Highest-blast-radius mistake available:** a repo-wide "remove every SQLite mention" sed touching `crates/runtara-core/migrations/postgresql/{013:11,016:6,017:17}`. Every existing production database then refuses to start. Only one of six surveys caught this. Bound every sweep with `':!crates/runtara-core/migrations/postgresql'`.

**Red-in-CI-only:** `ci.yml:415-438` `test-units` runs `cargo test --workspace --lib` with **no `services:` block** and `needs: build` (restoring a `target/` built under `GATE_FEATURES` — a different feature set). If Phase 2 goes the Postgres route, that job's decision and the ci.yml edit must land in the *same* commit; a developer with a live local Postgres will not reproduce the failure. The comment at `ci.yml:397-406` ("`--workspace --lib` needs no Postgres, Valkey or Docker (verified: 3,508 tests, 0 failures on a bare machine)") becomes false and must be rewritten with the new count. *(Two surveys pointed the prose edit at `:435-437`, which contains different text.)* Under the mock route the comment stays true and needs no edit.

**Red-locally-only, the inverse:** `main.rs:28` calls `dotenvy::dotenv()` and `.env:8` points at the live dev DB. Any new fixture reading `RUNTARA_DATABASE_URL` instead of `TEST_RUNTARA_DATABASE_URL` (core) / `TEST_ENVIRONMENT_DATABASE_URL` (environment) silently targets `localhost:5432/runtara` on a dev machine and nothing in CI.

**Also `test-core` goes red at compile time, not just `test-units`.** `parity_harness.rs:362` and `:372` are **not** behind `#[cfg(feature = "db-integration-tests")]` (unlike `:360,363,365,367,370`), so deleting `SqlitePersistence` breaks *every* form of `cargo test -p runtara-core`, including `ci.yml:291`. Four surveys framed test-core purely as a template to copy from.

**The git pre-commit hook is not the CI gate.** `.git/hooks/pre-commit` (untracked; no committed `.githooks/`, `core.hooksPath` default) runs `cargo clippy --workspace --all-targets -- -D warnings` **without `--features "$GATE_FEATURES"`**, so it never compiles `crates/runtara-environment/tests/heartbeat_monitor_test.rs:258` — a full 27-method `impl Persistence` behind `required-features`. It also **early-exits when no `.rs` file is staged**, meaning the commit deleting `migrations/sqlite/*.sql` runs zero checks. Do not treat a green pre-commit as a green gate.

**Coverage accounting, stated honestly in the commit message.** Today `cargo test --workspace --lib` carries ~85 database-backed SQLite tests (53 core + 32 environment). CI's DB-backed core case count drops 46+1 → 18+1+ported. Twelve pure-unit persistence tests survive regardless (`dialect/postgres.rs:366-411` ×5, `common/row.rs` ×4, `common/filters.rs` ×2, `common/error.rs` ×1) — *coverage does not go to zero*, contra one survey. State the new number so a future reader can tell deliberate movement from silent loss.

**Three surviving mock `Persistence` impls** — `instance_handlers/mock_persistence.rs:152`, `runtime.rs:391`, `runtara-sdk/src/backend/embedded.rs:620`, plus `runtara-environment/tests/heartbeat_monitor_test.rs:258` (four). **Keep the trait.** Removing SQLite leaves 1 real impl + 4 mocks; collapsing to a concrete `PostgresPersistence` would trade a zero-cost abstraction for a hard database dependency in nearly every unit test across three crates. SQLite removal does not motivate touching it. *(`claim_sleeping_instance` has a default body at `persistence/mod.rs:566-569` and none of the four override it — one survey's ordering constraint listing three "implementors" was citing `get_sleeping_instances_due` line numbers.)*

**No semver constraint.** Verified: no `cargo publish` step in `.github/workflows/` or any of the 11 files in `scripts/`; releases are GitHub bundles + GHCR only. Three crates additionally set `publish = false` (`runtara-text-parser:9`, `runtara-object-store:14`, `runtara-connections:9`). Deleting `pub use SqlitePersistence`, `pub mod sqlite`, `pub static SQLITE`, `pub async fn run_sqlite`, `pub struct SqliteDialect` carries zero deprecation obligation.

**No binary-size win — do not promise one.** `nm target/debug/runtara-server` returns 380,748 symbols and **zero** `sqlite3` matches; the linker never pulls `sqlite3.o` in. The shipped bundle and Docker image change by ~0 bytes. The real win is ~13 s of serial CPU per cold build/profile (measured: 12.09 s for the single 8.7 MiB `sqlite3.c` TU at `-O2`; 12.93 s A/B in an isolated cargo project) — not on the critical path (`docs/crates-structure.md` puts that at the 99.2 s wasmtime/cranelift chain), and not sccache-served (`release.yml`'s `CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER` wraps workspace rustc only, never `cc`).

---

## Verification gate

Must all pass before this is called done.

```bash
cd /Users/volodymyrrudyi/work/runtara
export SQLX_OFFLINE=true

# 1. Nothing but intentional survivors mentions SQLite.
git grep -in sqlite -- crates/ ':!crates/runtara-core/migrations/postgresql'
git grep -c SYN-394 -- crates/                       # expect 0
git diff --stat main -- crates/runtara-core/migrations/postgresql/   # MUST be empty

# 2. The C dependency actually left the compiled graph.
#    (Cargo.lock still listing libsqlite3-sys/sqlx-sqlite is CORRECT, not a failure.)
cargo tree -p runtara-core -e normal -i libsqlite3-sys   # "nothing to print"
cargo tree -p runtara-core -e normal -i sqlx-sqlite      # "nothing to print"
cargo tree -p runtara-server -i libsqlite3-sys -e normal # "nothing to print"

# 3. Fresh build — the migrations/sqlite deletion can pass from incremental cache.
cargo clean -p runtara-core
cargo build --workspace --all-targets

# 4. THE CI GATE (per the owner's standing rule; -p scoping hides trait breakage).
#    Prereq: crates/runtara-server/frontend/dist/ must exist or build.rs:61-73 hard-errors.
cargo clippy --workspace --all-targets \
  --features "runtara-server/embed-ui,runtara-server/db-integration-tests,runtara-server/valkey-integration-tests,runtara-server/valkey-tls-integration-tests,runtara-object-store/db-integration-tests,runtara-environment/db-integration-tests,runtara-core/db-integration-tests,runtara-component-host/component-integration-tests,runtara-workflows/direct-wasm-integration-tests" \
  -- -D warnings

# 5. Tests, all three lanes.
cargo test --workspace --lib
TEST_RUNTARA_DATABASE_URL=postgres://postgres:postgres@localhost:5432/runtara_test \
  cargo test -p runtara-core --features db-integration-tests -- --test-threads=1
TEST_ENVIRONMENT_DATABASE_URL=postgres://postgres:postgres@localhost:5432/runtara_test \
  cargo test -p runtara-environment --features db-integration-tests -- --test-threads=1

# 6. Rustdoc hygiene (local only — no cargo doc job in CI).
cargo doc -p runtara-core --no-deps 2>&1 | grep -i warning   # expect none

# 7. e2e — the only coverage of the standalone core binary. Not run by any workflow.
bash e2e/test_core_sigterm_drain.sh
bash e2e/test_core_http_status_codes.sh
psql -U smo_worker -h localhost -tAc \
  "SELECT datname FROM pg_database WHERE datname LIKE 'runtara_e2e_core%'"   # expect empty
```

**Manual checks no command covers:**
- `crates/runtara-core/src/main.rs` rejects `RUNTARA_DATABASE_URL=sqlite:///tmp/x.db` with an error naming the variable and the required scheme — not a raw sqlx parse error.
- `CHANGELOG.md` `### Removed` sits between `### Changed` and `### Fixed`.
- The four restored assertions in `runtime_host.rs` actually **fail** if you revert the fix they guard.

---

## Decisions still owned by you

1. **Phase 2 route** — Postgres+gate (smaller diff, 32 tests leave the free local loop) vs. promoted MockPersistence (keeps them free, costs ~10 doc comments to satisfy `deny(missing_docs)` + 3 method implementations). Recommendation: mock for the three `src/` modules, gate for `embedded_runner_test.rs`.
2. **Do the two core e2e scripts stay in `run_all.sh`** now that they need Postgres, or move behind an opt-in flag? They exist partly *because* they were dependency-free, and no CI job runs them.
3. **`migrations/postgresql/` flattening** — decide before writing the `add-migration` skill edit; the skill must cite a path that exists.
4. **Does `db-integration-tests` stay?** With SQLite gone there is no docker-free persistence test at all. Keeping it as-is is the status quo and already CI-gated (`ci.yml:27,291`); folding it into `default` would make `cargo test -p runtara-core` require a database.
5. **`enforce_unacked_cancel` / `cancel_pending` guards** — keep as defence-in-depth (recommended: correct, free, and removing them makes correctness depend on backend SQL forever) or delete along with `an_acknowledged_cancel_does_not_re_cancel_a_finished_run`, which becomes a tautology either way.