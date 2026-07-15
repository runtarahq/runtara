# Fix Plan: Retry-backoff re-invokes already-attempted agent calls after a drain/restart mid-retry

**Status:** ✅ Implemented (2026-07-14) — Approach 1, **err-only envelopes** (§6.4). All checks green (503 emitter unit + 71 direct-wasm integration incl. the 106-fixture smoke battery + 168 stdlib unit, clippy clean).
**Date:** 2026-07-14
**Bug:** Priority Medium · `bug` `workflow-engine` `durability` `idempotency`
**Scope of fix:** the durable **Agent** retry loop only (`direct_wasm` emitter). Split/Embed retry are analyzed and explicitly deferred (§8).

**As-built notes:**
- Shipped the **err-only** variant (§6.4): a per-attempt envelope is written only for *failed* attempts; success rides the existing outer step checkpoint. This dissolves the HIT-ok output-clobber (correctness fix #2 never applies) and keeps the common happy path at +1 read.
- Two new stdlib funcs (no WIT-runtime change): `agent-attempt-result-key`, `agent-attempt-envelope` — regenerate `bindings.rs` via `wit-bindgen rust crates/runtara-workflow-wit/wit/stdlib --runtime-path wit_bindgen_rt --format` (or just `cargo component build`, which does it).
- The retry loop is now a unified prologue (`agent.rs`) + shared err-arm; the non-durable path is behavior-identical (`HIT_FLAG` never read on that path). New locals 110–115 (`compile.rs`), local group bumped 6→12 (`core_module.rs`).
- Acceptance test `direct_wasm_execute_durable_agent_retry_replays_attempts_across_resume` covers the tripwire (per-attempt persistence, RED on unfixed), the strong no-re-invoke assertion (resume fires only the frontier attempt: 1, not 3), and success-output integrity. Envelope round-trip is a stdlib unit test.
- Split/While per-iteration isolation (Test B) is implemented: `direct_wasm_execute_durable_agent_retry_per_iteration_isolation_across_resume` runs a durable agent inside a 2-item Split, asserts the two iterations invoke independently (`llm_requests == 6`, not fewer), that each owns two distinct `…::[i]::attempt::N` keys (2/2), and that resume replays each iteration's own attempts (only 2 frontiers fire). This guards the loop-index folding against a future collision regression.

---

## 0. TL;DR / verdict

When a durable agent step is mid retry-backoff and the environment drains/restarts, resume **replays from the start**, the step's success checkpoint misses (it only checkpoints on success), the in-memory attempt counter resets to `1`, and the retry loop **re-invokes the agent for every attempt that already ran**. For a non-idempotent call with a partial side effect, that double-fires.

**Fix (Approach 1 — per-attempt invoke durability):** add an **inner per-attempt result checkpoint** keyed `"{cache_key}::attempt::{N}"`, written once right after each *failed* attempt's invoke. On replay, `get-checkpoint` it **before** the invoke; a hit short-circuits the invoke and replays the stored failure through the existing retry state machine. Reuse the existing `get-checkpoint` / `checkpoint` host imports — **no WIT change**.

Three non-obvious facts the investigation established that reshape the naive fix:

1. **The backoff sleep re-runs at full duration on replay.** The guest uses the HTTP SDK backend exclusively; its `/sleep` lands in core `handle_sleep`, which unconditionally `tokio::sleep`s the full `duration_ms` with no elapsed/deadline check ([checkpoint.rs:217-246](crates/runtara-core/src/instance_handlers/checkpoint.rs:217)). The "skip if already elapsed" logic lives only in the SDK *embedded* backend, which WASM never uses. **So the fix must also gate the backoff sleep on the per-attempt hit** — do not rely on the per-attempt sleep key skipping itself.
2. **You cannot re-derive the retry decision on replay.** The agent classification formula differs from the workflow one, and the workflow path reads `AUTO_RETRY_ON_429` from the environment at classify time. The per-attempt envelope must persist the **already-computed** classification bits, not the raw error alone.
3. **`record_retry_attempt` is write-only audit** with zero readers and a Postgres-only schema; it is not a usable resume cursor (rules out the "restore the counter" variant as a backend-neutral fix).

Adversarial verification confirmed the approach is architecturally sound and surfaced two control-flow correctness holes and a non-discriminating acceptance test, all fixable within Approach 1. The refinements are folded into this plan (§6, §10).

---

## 1. Root cause (verified against code)

The agent step lowering wraps the invoke in an optional retry loop inside an optional durable-checkpoint `if/else` (`emit_agent_plan`, [agent.rs:44-392](crates/runtara-workflows/src/direct_wasm/compile/agent.rs:44)):

- **Outer success checkpoint.** `emit_checkpoint_lookup(cache_key)` opens the "if hit" arm at [agent.rs:175](crates/runtara-workflows/src/direct_wasm/compile/agent.rs:175); `Else` at [agent.rs:183](crates/runtara-workflows/src/direct_wasm/compile/agent.rs:183) holds the retry loop; `emit_checkpoint_save(cache_key)` stores the **success output** at [agent.rs:317](crates/runtara-workflows/src/direct_wasm/compile/agent.rs:317). Keyed by the plain cache key; **saved only on success.**
- **In-memory attempt counter.** `DIRECT_AGENT_RETRY_ATTEMPT_LOCAL` is reset to `1` on each entry ([agent.rs:194-195](crates/runtara-workflows/src/direct_wasm/compile/agent.rs:194)) — a WASM local, never persisted. The rate-limit wait budget local resets similarly.
- **The invoke is inside the loop with no guard.** `emit_agent_invoke` at [agent.rs:198](crates/runtara-workflows/src/direct_wasm/compile/agent.rs:198); on retryable failure the loop advances the attempt, computes the delay, does a **durable per-attempt sleep** (`{cache_key}::retry_sleep::{N}`, [direct_json.rs:2256](crates/runtara-workflow-stdlib/src/direct_json.rs:2256)), and records an audit row. Nothing short-circuits the invoke on replay.
- **Resume = replay-from-start.** On drain, a straggler blocked in the backoff sleep is force-stopped, its `Store` torn down, and it is persisted `suspended + shutdown_requested + sleep_until=now`; the wake scheduler relaunches it and it re-runs from the top (`RUNTARA_CHECKPOINT_ID` is written into the guest env but never read; relaunch is driven solely by `sleep_until`). Confirmed in `runtime.rs` drain path, `wake_scheduler.rs`, and `recovery.rs`. Crash/kill reaches the same path via orphan recovery.

**Net:** on resume the outer cache-key lookup misses (step never succeeded), the loop re-enters at attempt 1, and `emit_agent_invoke` re-fires for attempts `1..k` that already ran — **and** each per-attempt backoff sleep re-runs at full duration (fact #1 above).

## 2. Why it's latent today

Retry backoff **blocks** (the guest stays resident) — the instance does not voluntarily suspend during backoff; status stays `running`. Absent a restart, the whole retry loop completes in one resident run and each attempt executes exactly once. The double-fire only manifests when a drain/crash lands **inside** the backoff window — more likely with long rate-limit `retry-after` waits, and amplified by repeated drains during a long backoff.

This is exactly **Blocker B** of [docs/unify-agents-workflows-plan.md](docs/unify-agents-workflows-plan.md) §3. Fixing it is a prerequisite the unify migration's Stage 2 needs before routing retry backoffs through a suspend/replay path.

---

## 3. Chosen approach & rejected alternatives

**Approach 1 — inner per-attempt result checkpoint (chosen).** Keyed `"{cache_key}::attempt::{N}"`, written once per *failed* attempt. On replay the structured loop naturally walks `N = 1,2,3…`, hitting cached failures (skip invoke) until the first miss (the frontier attempt), which invokes fresh. It composes with the existing outer success checkpoint and the existing per-attempt durable sleep, needs no mutable cursor, and re-accumulates the rate-limit budget deterministically by replaying each stored failure through the existing condition logic.

**Rejected:**

| Alternative | Why rejected |
|---|---|
| **Approach 2** — persist/restore a single `{attempt, budget}` cursor | Incompatible with the write-once checkpoint primitive (`handle_checkpoint` never overwrites an existing key — [checkpoint.rs:57-88](crates/runtara-core/src/instance_handlers/checkpoint.rs:57)); an in-place mutable cursor is impossible without per-attempt keys + a highest-N scan that structured WASM control flow makes awkward. Leaves a residual double-invoke window (crash after an attempt's side effect but before the counter persists) and would need explicit budget persist/restore that Approach 1 gets for free. |
| **Re-derive** classification on replay from the raw stored error (as embed does) | Agent formula (`retryable = capability-declared-retryable && category!='permanent'`, [direct_json.rs:2361](crates/runtara-workflow-stdlib/src/direct_json.rs:2361)) ≠ workflow formula ([direct_json.rs:4392](crates/runtara-workflow-stdlib/src/direct_json.rs:4392), ignores declared-retryable), and the workflow path reads `AUTO_RETRY_ON_429` from env at classify time. Re-classifying can silently change the decision. Persist the computed bits instead. |
| **Rely on the durable per-attempt sleep to skip on replay** | False for the guest's backend — core `handle_sleep` always sleeps the full duration (fact #1). Must gate the sleep on the per-attempt hit explicitly. |
| **New `get-retry-attempt` WIT method / reuse `save_retry_attempt` rows** | Unnecessary: `get-checkpoint` + `checkpoint` suffice. `save_retry_attempt` is write-only audit with zero readers, stores empty state, and its columns exist only in the Postgres schema (absent on SQLite) — not backend-neutral. |
| **Triplicate the mechanism into Split/Embed** | Their retryable unit is a *subgraph*, not a single external invoke; leaf side effects are bounded by the leaves' own per-step checkpoints (see §8). An `::attempt::` envelope there would be an optimization, not a correctness fix. |

---

## 4. Keying

Per-attempt result key = `"{cache_key}::attempt::{N}"`.

- `cache_key` = the deterministic agent step key from `agent_cache_key` ([direct_json.rs:3226-3255](crates/runtara-workflow-stdlib/src/direct_json.rs:3226)): `"{prefix}::agent::{agent_id}::{capability_id}::{step_id}[::[loop_indices]]"`, derived only from static identifiers + scope variables — **no timestamp/counter/randomness**, so it is stable across replay-from-start with the same `instance_id`.
- `N` = the attempt number at invoke time (before advance), re-derived identically by the loop replay.

**Non-collision proof.** Checkpoints are uniquely keyed by `(instance_id, checkpoint_id)` (UNIQUE on both backends) and looked up by the full string. Four disjoint namespaces by literal infix:

| Purpose | Key |
|---|---|
| Outer success | `{cache_key}` (no suffix) |
| Durable per-attempt sleep | `{cache_key}::retry_sleep::{N}` |
| Write-only audit | `{cache_key}::retry::{N}` |
| **New per-attempt result** | `{cache_key}::attempt::{N}` |

`::attempt::` shares no infix with `::retry_sleep::` or `::retry::`; the audit scan's `LIKE '…::retry::%'` matches none of them. Choosing `::attempt::` (not `::retry::`) specifically avoids cross-contaminating that scan.

**Loop safety.** Because `cache_key` already folds `_loop_indices`, the same agent step inside a Split/While gets distinct `…::[i]::attempt::N` keys per iteration — no cross-iteration collision. (Contrast the Delay step, which keys its sleep by bare `step_id` and *does* collide across iterations — do **not** copy that pattern here. Guard with a test — §10.)

---

## 5. Envelope format

Small tagged byte blob stored in the checkpoint `state` (arbitrary `BYTEA`/`BLOB`, only constraint: non-empty — `handle_checkpoint` treats empty state as a read-only probe and refuses to persist it, [checkpoint.rs:95-104](crates/runtara-core/src/instance_handlers/checkpoint.rs:95)). The leading tag byte guarantees non-empty.

```
err envelope (tag 0x00):
  [tag:u8=0x00]
  [retryable:u8]              // already-computed agent-formula bit
  [rate_limited:u8]           // already-computed bit
  [retry_after_tag:u8]        // option discriminant
  [retry_after_ms:u64_le]     // RAW captured retry-after (pre-clamp) — see §6
  [error_payload_bytes …]     // the agent error-info JSON (for exhausted-error body + audit)
```

The **ok envelope is optional** (see §6 "success-attempt policy"). If adopted, `tag 0x01` + raw output `list<u8>`.

`retry_after_ms` **must be the RAW captured value** (as set by `emit_agent_capture_retry_sleep`), not the clamped delay that `emit_agent_retry_delay` writes later into the same shared local. The budget accumulation in `emit_agent_retry_condition` reads the raw value; storing the clamped one would make replay diverge whenever `retry_after > max_delay`. Encode+save therefore run **before** `emit_agent_retry_delay` executes for that attempt (naturally true — the save lives in the prologue, the delay in the err-arm).

---

## 6. The mechanism in the emitter

All new logic is gated on `durable_checkpoint == true` **and** `max_retries > 0` (the existing loop gate at [agent.rs:193](crates/runtara-workflows/src/direct_wasm/compile/agent.rs:193)). Non-durable and `maxRetries:0` steps emit byte-identically to today.

### 6.1 Per-attempt prologue (top of the retry `Loop`, before the invoke)

Insert a **self-converging, balanced `If/Else`** between the `Loop` opener ([agent.rs:197](crates/runtara-workflows/src/direct_wasm/compile/agent.rs:197)) and `emit_agent_invoke` ([agent.rs:198](crates/runtara-workflows/src/direct_wasm/compile/agent.rs:198)):

1. Build `attempt_key = {cache_key}::attempt::{DIRECT_AGENT_RETRY_ATTEMPT_LOCAL}` (new stdlib helper).
2. `runtime_get_checkpoint(attempt_key)`; set `DIRECT_AGENT_ATTEMPT_HIT_FLAG` from the found bit (stash immediately — retptr is shared scratch).
3. **HIT** (a stored failure): decode the envelope (new stdlib decode-view) and `LocalSet`:
   - `DIRECT_AGENT_RETRY_ERROR_PTR/LEN` ← payload ptr/len,
   - `DIRECT_AGENT_RETRYABLE_LOCAL` ← retryable, `DIRECT_AGENT_RATE_LIMITED_LOCAL` ← rate_limited,
   - `DIRECT_AGENT_RETRY_SLEEP_TAG_LOCAL` ← retry_after_tag, `DIRECT_AGENT_RETRY_SLEEP_MS_LOCAL` (idx 16) ← **raw** retry_after_ms.
   Set `DIRECT_AGENT_ATTEMPT_ERR_FLAG = 1`. **No invoke.**
4. **MISS**: `emit_agent_invoke` as today; set `DIRECT_AGENT_ATTEMPT_ERR_FLAG` from the invoke result tag; then **branch on the tag**:
   - **err**: run the *relocated* `emit_agent_capture_retry_sleep` + `emit_agent_retry_error_info` (moved out of [agent.rs:209-215](crates/runtara-workflows/src/direct_wasm/compile/agent.rs:209)) so the error locals are populated, encode the err envelope, and **bare** `runtime_checkpoint(attempt_key, envelope)` (not `emit_checkpoint_save`).
   - **ok**: do **not** run `emit_agent_retry_error_info` (it reads err offsets and would clobber the output). Leave the invoke result in retptr for the existing success path. (Optionally encode+save an ok envelope — see policy below.)

The prologue opens and closes entirely before the existing err-arm `If`, so **`Br(2)` at [agent.rs:246](crates/runtara-workflows/src/direct_wasm/compile/agent.rs:246), `Br(1)` at [agent.rs:276](crates/runtara-workflows/src/direct_wasm/compile/agent.rs:276), and `failure_target.nested(3)` / `handled_target.nested(3)` at [agent.rs:271-272](crates/runtara-workflows/src/direct_wasm/compile/agent.rs:271) are unchanged.**

> **Correctness fix #1 (blocker, from verification):** the MISS path **must branch on the invoke result tag**. Running `emit_agent_retry_error_info` unconditionally corrupts the output of every successful terminal attempt (it reads the error struct from a retptr that actually holds an ok `list<u8>` and overwrites `output_ptr/len`).

### 6.2 Err-arm branches on the flag

Change the existing err-arm `If` at [agent.rs:208](crates/runtara-workflows/src/direct_wasm/compile/agent.rs:208) to branch on `DIRECT_AGENT_ATTEMPT_ERR_FLAG` instead of the raw retptr tag (on a HIT there is no fresh retptr). The err-arm body is otherwise unchanged: it evaluates `emit_agent_retry_condition` (which re-accumulates the rate-limit budget from the restored `SLEEP_TAG`+`SLEEP_MS`), advances the attempt, and on exhaustion falls to `emit_agent_invoke_error_body_from_info` (which reads the restored error payload — correct on replay).

### 6.3 Gate sleep/record/delay on the hit

In `agent_retry.rs`, gate `emit_agent_retry_sleep`, `emit_agent_record_retry_attempt`, **and** `emit_agent_retry_delay` on `!DIRECT_AGENT_ATTEMPT_HIT_FLAG`:

- On a fresh **MISS-err**: sleep + record + delay run exactly as today.
- On a replayed **HIT-err**: **skip** the durable sleep (fact #1 — it would re-sleep the full backoff), skip the audit record, and skip the delay recompute (its output is never consumed since the sleep is skipped). Still run condition + advance so the counter advances and the budget re-accumulates.

Because the per-attempt **save already happened in the prologue** (before the err-arm's sleep path clobbers `DIRECT_AGENT_RETRY_ERROR_PTR/LEN` with the sleep key at [agent_retry.rs:129-131](crates/runtara-workflows/src/direct_wasm/compile/agent_retry.rs:129)), the existing scratch reuse is unaffected.

### 6.4 Success-attempt policy (write-cost decision)

**Recommended: err-only envelopes.** Save a per-attempt envelope **only for failed attempts**; rely on the existing **outer** success checkpoint for the success case. Consequences:

- The success attempt is always a fresh MISS on replay → invoked once. It re-invokes *at most once* only if a crash lands in the microscopic window between invoke-success and the outer save — **identical to the pre-existing exposure of any durable `maxRetries:0` step**, and not the drain-during-backoff scenario the bug describes.
- **Dissolves correctness fix #2** entirely: there is no HIT-ok path, so `load_agent_retptr_list` at [agent.rs:275](crates/runtara-workflows/src/direct_wasm/compile/agent.rs:275) always runs after a real invoke (never over stale scratch) and needs no change.
- **Lowest write cost:** the common durable-retry step that succeeds on attempt 1 adds only **one read** (`get`, miss) over today; genuine retries add one `checkpoint` write per *failed* attempt (exactly the attempts that occurred).

**Optional hardening (ok envelopes):** to make even the successful attempt strictly once-per-lifecycle, also encode+save an ok envelope and, on a HIT-ok replay, **gate `load_agent_retptr_list` at [agent.rs:275](crates/runtara-workflows/src/direct_wasm/compile/agent.rs:275) on `!HIT`** (or repopulate the retptr ok offsets from the decoded payload). This closes the tiny success→outer-save window at the cost of one extra write per attempt and a HIT-ok replay path (which must then be tested — §10). Adopt only if strict once-semantics for the success attempt is required.

> **Correctness fix #2 (blocker, from verification):** applies **only if ok envelopes are adopted.** The err-only recommendation avoids it.

### 6.5 New locals

Add i32 locals by bumping the trailing `(6, ValType::I32)` group in [core_module.rs:436](crates/runtara-workflows/src/direct_wasm/compile/core_module.rs:436) and defining constants at indices 110+ in **`src/direct_wasm/compile.rs`** (the sibling file, *not* `compile/compile.rs`; highest existing index is 109):

- `DIRECT_AGENT_ATTEMPT_HIT_FLAG`, `DIRECT_AGENT_ATTEMPT_ERR_FLAG`, `DIRECT_AGENT_ATTEMPT_KEY_PTR`, `DIRECT_AGENT_ATTEMPT_KEY_LEN`.

Reuse `DIRECT_AGENT_RETRY_SLEEP_MS_LOCAL` (i64, idx 16) for `retry_after_ms` — no new i64 local. (Bumping the shared `run` function's local group adds a few unused locals to every workflow's core module; A/B parity compares runtime outputs, not module bytes, so this doesn't perturb parity — confirm no byte/wat golden exists before release.)

---

## 7. Files to change

**Emitter (`crates/runtara-workflows/src/direct_wasm/compile/`):**
- `agent.rs` — per-attempt prologue (§6.1); err-arm branches on the flag (§6.2).
- `agent_retry.rs` — `!HIT` gate on sleep/record/delay (§6.3); relocate capture/error-info into the prologue.
- `core_module.rs` — bump the trailing i32 local group (§6.5).
- `core_imports.rs` — wire `Option<u32>` index fields for the 3 new stdlib helpers; **reuse** the existing `runtime_get_checkpoint` / `runtime_checkpoint` indices.
- `../compile.rs` — declare the 4 new local-index constants (110-113).

**Stdlib (`crates/runtara-workflow-stdlib/src/`):**
- `direct_json.rs` — `agent_attempt_result_key(cid, n) -> "{cid}::attempt::{n}"`; `agent_attempt_envelope_encode`; `agent_attempt_envelope_decode_view` (fixed-offset struct read via `push_retptr_*`, mirroring the existing `DIRECT_AGENT_RETRY_INFO_*` read at [agent_retry.rs:201-208](crates/runtara-workflows/src/direct_wasm/compile/agent_retry.rs:201)).
- `lib.rs` + `crates/runtara-workflow-wit/src/lib.rs` — expose the 3 helpers as stdlib imports.

**WIT:** none. `get-checkpoint` + `checkpoint` already exist and suffice.

Rebuild stdlib with `RUNTARA_ONLY_WORKFLOW_COMPONENTS=1`; revert incidental `bindings.rs` reformatting churn (only keep changes to interfaces you actually touched).

---

## 8. Scope: Agent-only; Split/Embed deferred

The reported unbounded double-fire has exactly **one** source: the leaf agent network invoke, whose only success persistence is the whole-step checkpoint. `split_retry.rs` and `embed_retry.rs` share the *structural* pattern (counter reset to 1, success-only whole-result checkpoint, per-attempt durable sleep via the same `stdlib_retry_sleep_key`) but differ **semantically**:

- Their retryable unit is a **subgraph**, and the retry-attempt local never enters any cache-key or scope construction (verified: `*_RETRY_ATTEMPT_LOCAL` appears only in retry helpers + frame save/restore). So nested/child durable steps keep stable cache keys across attempts and across resume, and a **succeeded leaf hits its own per-step checkpoint and does not re-fire.**
- `split_retry.rs` / `embed_retry.rs` make **no external invoke themselves**.

Therefore an `::attempt::` envelope there would be a redundant optimization, not a correctness fix. **Do not include them.**

**Optional fast-follow (separate change, not a blocker):** (a) a per-attempt whole-result checkpoint so resume skips re-running an already-succeeded attempt's subgraph; (b) persist/restore the attempt counter + rate-limit budget so the retry *policy* isn't silently refreshed on each drain. This budget-refresh drift affects all three paths; **the agent fix here already cures it for agents** via error replay.

**Blocker B relationship (complementary, not conflicting).** [unify-agents-workflows-plan.md](docs/unify-agents-workflows-plan.md) §3 names two sanctioned prerequisites: "keep retry backoffs on the blocking path (distinct import) OR checkpoint the agent invoke per-attempt." Approach 1 is literally the per-attempt option for the agent path, and it does **not** reroute the shared `runtime_durable_sleep_checkpoint` import through suspend — it only gates whether the agent caller *calls* the sleep. Caveat: this unblocks only the agent caller; `split_retry.rs:194` and `embed_retry.rs:198` still share that import, so Stage-2 conversion of the sleep import to a suspend point remains blocked until those get per-attempt checkpointing or a distinct blocking import.

---

## 9. Edge cases

- **Non-durable path** (`durable_checkpoint=false`): prologue not emitted; uses `runtime_blocking_sleep` (`thread::sleep`, no persistence); re-executes on replay by design — unchanged, correct.
- **`maxRetries:0`**: whole loop is gated out ([agent.rs:193](crates/runtara-workflows/src/direct_wasm/compile/agent.rs:193)); byte-identical to today. (Default `maxRetries` is 0 for single-shot AiAgent; most steps are unaffected.)
- **Exhaustion / non-retryable final failure:** the final failed attempt `N` is saved as `::attempt::N` before the loop breaks to the failure path, so even the terminal invoke is not re-fired on a post-failure/pre-onError-routing drain. Resume replays all `::attempt::` hits, the condition exhausts identically, and routes to the same onError target. No outer success checkpoint is written — correct.
- **Rate-limit budget:** replaying each stored failure through `emit_agent_retry_condition` re-accumulates `DIRECT_AGENT_RATE_LIMIT_WAIT_TOTAL_LOCAL`, honoring the total-wait cap across resume (fixes the pre-existing budget-reset drift for agents).
- **Rate-limit politeness after resume** (accepted trade-off): because the sleep is skipped on hits, the backoff immediately preceding the first fresh (MISS) attempt on resume is not re-slept, so that attempt may fire before the server's `retry-after` fully elapses. This is timing/politeness only — a re-429 is a harmless retry, not a duplicated side effect. Optional precise variant: peek `::attempt::{N+1}` to gate the sleep (sleep only when the next attempt was *not* already served from checkpoint).
- **onError routing:** prologue is fully enclosed, so `Br` depths and `nested(3)` targets are untouched.
- **AiAgent single-shot reuse (`output_fn`):** the success path is unchanged (err-only policy); the existing `output_fn` transform runs after the outer save exactly as today. AiAgent *tool-loop* turn durability (`ai.turn.{N}`) is orthogonal and does not collide with `::attempt::{N}` — but add a tool-loop retry case in a follow-up test to confirm no double-wrapping.
- **Debug/breakpoint fidelity:** on a HIT the invoke is skipped, so its per-invoke debug event naturally isn't emitted; ensure replayed attempts don't emit misleading fresh AI/tool-call debug events and that per-iteration scope events keep `scope_id`/`loop_indices`.
- **`handle_checkpoint` guards:** requires status `running` (satisfied in the running replay context) and treats empty state as a probe (the tag byte keeps envelopes non-empty). Route the save through the `checkpoint` import (load-first idempotent), **never a raw `save_checkpoint`** — SQLite's plain-INSERT would UNIQUE-violate on any accidental double-save (Postgres upserts; the divergence is deliberate).
- **`instances.checkpoint_id` thrash:** saving `::attempt::{N}` moves `instances.checkpoint_id` to that key; inert today because relaunch is checkpoint-less and driven solely by `sleep_until`. Add a comment; guard any future recovery logic that expects the success key.

---

## 10. Acceptance tests

Author as new `#[test]`s in [crates/runtara-workflows/tests/direct_wasm_execute.rs](crates/runtara-workflows/tests/direct_wasm_execute.rs) (`required-features=["direct-wasm-integration-tests"]`, embedded executor, `--test-threads=1`, staged agent components) — **not** a full-server e2e. "Resume" is simulated exactly like `direct_wasm_execute_ai_agent_loop_replays_completed_turns_without_rebilling` (:2826): replay-from-start against a **preloaded `/checkpoint` store** keyed by the same `instance_id` (`RUNTARA_INSTANCE_ID == workflow_id`), using the mock's save-or-return-existing semantics. Counter: `CapturedRun.llm_requests` (one per POST `/llm-proxy`).

**Test A — discriminating replayed-success (the core acceptance test).** Fixture: `single_shot_ai_agent_graph_json` with `maxRetries:5, retryDelay:10`.
- **Run 1** (generate checkpoints): script `[llm_http_500(), llm_http_500(), llm_ok("a")]` → attempts 1,2 fail retryably (err envelopes saved), attempt 3 succeeds. Assert `status_success`, `llm_requests.len() == 3`, and **harvest is non-empty** — `checkpoint_id.contains("::attempt::") && !state.is_empty()` (this assertion alone is RED on unfixed code, which persists no such keys).
- **Run 2** (resume mid-backoff after attempt 2): preload **only** `::attempt::1` and `::attempt::2` (drop `::attempt::3` and the outer success key). Script `[llm_ok("recovered")]`. Assert `status_success`, output `answer == "recovered"`, and `llm_requests.len() == 1` (attempts 1,2 replayed from checkpoint → not re-invoked; only attempt 3 fires).

> **Test-design fix (from verification):** a Run-2 that only asserts `llm_requests == 1` does **not** discriminate fixed from unfixed — with a 1-entry ok script, unfixed code also stops after one call. Discrimination comes from the **harvest-non-empty tripwire** (unfixed persists nothing) plus, for extra strength, preloading `::attempt::1` as a **replayed success** and asserting the answer is **sourced from the envelope, not the live script** (fixed → 0 fresh invokes; unfixed → 1 fresh invoke from the script). Keep both the harvest-non-empty and the invoke-count assertions distinct.

**Test B — Split/While per-iteration isolation.** A durable agent inside a 2+-iteration Split (or While), drain mid-backoff on one iteration; assert each iteration fires its own network calls exactly once and no iteration is short-circuited by another's envelope (distinct `…::[i]::attempt::N` keys harvested). Guards against a future refactor keying the attempt by bare `step_id`.

**Test C — envelope round-trip unit test.** `encode → decode_view` for both tags, asserting the `u64_le` `retry_after_ms` offset and payload offset survive exactly. An off-by-one here silently corrupts restored retry decisions.

**Test D — terminal-success output integrity.** A durable retry that fails N times then succeeds; assert the final output equals the invoke's ok result (guards correctness fix #1 — MISS-path must not run `error_info` on success). Test A's Run 1 already exercises MISS-ok terminal success once the tag-branch gate is in place.

**Test E (only if ok envelopes adopted)** — HIT-ok replay: preload a **successful** `::attempt::` envelope and assert 0 fresh invokes with envelope-sourced output (guards correctness fix #2 and the [agent.rs:275](crates/runtara-workflows/src/direct_wasm/compile/agent.rs:275) gate).

Note: the mock `/sleep` route is a no-op and does **not** store a checkpoint, so a literal "drain during the sleep" isn't simulable in this harness; "crash at next invoke" (script exhaustion) leaves the identical persisted prefix (`::attempt::1..k`) and is the deterministic substitute. The real SIGTERM-drain/`sleep_until` half is covered separately by the server/core e2e path (see `reference_e2e_server_isolation`, `project_drain_suspend_recovery`).

---

## 11. Risks

| Risk | Mitigation |
|---|---|
| Hand-rolled envelope byte layout (`u64_le`, payload offset) off-by-one silently corrupts restored decisions | Direct `encode→decode` round-trip unit test (Test C) + replay test with `retry_after > max_delay` asserting identical terminal `::attempt::N` and identical accumulated budget |
| MISS path runs `error_info` on a successful invoke → output corruption (blocker) | Branch the MISS path on the invoke result tag (§6.1, fix #1); Test D |
| HIT-ok output clobber at [agent.rs:275](crates/runtara-workflows/src/direct_wasm/compile/agent.rs:275) (blocker) | **Avoided** by the err-only policy (§6.4); if ok envelopes are adopted, gate :275 on `!HIT` + Test E |
| Happy-path write amplification for durable `maxRetries>0` steps | err-only keeps the common (succeed-on-1) path at +1 read only; state the per-attempt cost; deferred ok envelopes are opt-in |
| Cross-iteration collision if a future refactor keys the attempt by bare `step_id` | Rely on `agent_cache_key` folding `_loop_indices`; Test B guards it |
| WASM dead-code CI gap (host clippy misses wasm32 dead code) | Verify via `scripts/build-agent-components.sh` before release |
| Emitter edits not caught by stdlib tests | Run `cargo test -p runtara-workflows` (the `--lib`/integration emitter tests), not just stdlib |
| `bindings.rs` regen churn | Rebuild with `RUNTARA_ONLY_WORKFLOW_COMPONENTS=1`; revert unrelated reformatting |

---

## 12. Implementation order

1. **Stdlib helpers** (key builder, encode, decode-view) + WIT-stdlib wiring + `core_imports` index fields. Land with Test C (round-trip) green.
2. **Locals** (`core_module.rs` group bump + `compile.rs` constants).
3. **Emitter** — prologue (§6.1) + err-arm flag branch (§6.2) + `!HIT` gates (§6.3), err-only policy (§6.4).
4. **Tests A–D** (and E if ok envelopes adopted). Confirm Test A Run-1 harvest-non-empty is RED before the emitter change and GREEN after.
5. **e2e-verify** the durable retry path end-to-end (compile → register → execute an agent step with `maxRetries>0`), then release checks (`build-agent-components.sh`, `cargo test -p runtara-workflows`).

---

## 13. Bottom line

The fix is a **surgical, backend-neutral addition** to one hot path: a per-attempt result checkpoint that makes each failed attempt durable, keyed in a collision-free namespace, reusing existing host imports with no WIT change. The three load-bearing subtleties — the backoff sleep re-running on replay, the un-reproducible retry classification, and the two control-flow correctness holes — are all resolved by (1) gating the sleep on the per-attempt hit, (2) persisting the computed classification bits, and (3) the err-only success policy that dissolves the HIT-ok hole and minimizes write cost. Split/Embed are correctly out of scope; the change is a clean partial prerequisite for the unify migration's Blocker B.
