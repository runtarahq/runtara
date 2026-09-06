# Selective isolation implementation record

Objective: implement [the full plan](selective-isolation-plan.md), preserving the
DSL and existing support, with small tested commits and final local-server E2E.
This record tracks implementation; the earlier research is not a completion claim.

## Starting state

- Fetched `origin/main` on 2026-09-06 and created `feat/selective-isolation`
  directly at `024c4c5c131314f98c67c58f3ac1be1b7a1e921e`.
- Upstream already contains audit PR #227 and environment ownership PR #228.
- Preserved research as `265cfe11`, benchmark tooling as `612283d0`, and the plan
  and measurements as `0867b13b`. Repository commit hooks passed.
- Fresh upstream-based baseline: 840 workflow tests passed; existing doctest and
  the manual benchmark ignored. Component host: 59 passed, manual benchmark ignored.

## Implemented and verified

### Process-local isolated task ownership

Committed as `c864d953`.

`runtara-component-host::isolated_tasks` provides owner-scoped non-reused handles,
fail-fast admission, aggregate retained-result accounting, cancellation/join/release,
and root shutdown. Completion versus cancellation has one locked publication
decision; a cancelled ready result cannot win merely because its future polls first.
The worker's owned future is dropped before the result becomes visible. Native
worker panic produces a terminal result rather than leaving join pending forever.

The Store runner must install the supplied token in its epoch callback before
instantiation. Actual guest-loop and infinite-initializer tests demonstrate this
contract, preserving a sibling and observing Store destruction before join. Pending
execution cleanup, pre-start cancel, duplicate/late commands, owner checks, stale
handles, capacity release, retained-result readers and reserved Vec capacity are
also tested. A root runner must explicitly await shutdown; Drop only requests
last-resort cancellation.

Verification: 13 new tests passed; complete component-host suite with both
integration/PoC features: **72 passed**, one manual benchmark ignored. Focused
all-target Clippy with both features and `-D warnings` passed.

This is the live ownership layer. It does not yet expose a WIT import, redirect
DSL Agent calls, implement persistent cancellation, or change a production default.

### Self-contained package contract

The optional `runtara-workflow-wit/isolation-package` module defines the shared
compiler/host catalog format. It appends one versioned custom section to a root
component, stores each child once by SHA-256, and binds package-local names to
those bytes. Parsing borrows verified child slices; it rejects overlapping bodies,
unknown versions, invalid references, duplicate bindings, trailing/unindexed data,
nested catalogs and native-code inputs. Bounds apply before decoding the index.
Existing components without a catalog remain unmodified.

Verification: 10 package tests plus the five existing WIT contract tests passed.
A component-host integration test packages 100 bindings to one child, validates
the package with Wasmtime and executes both its root and resolved child. Focused
all-target Clippy passed. The guest-only WIT dependency stays light: package codec
dependencies are behind the host/compiler feature.

This establishes packaging and validation; it does not yet select a compiler
backend or expose the catalog to running workflows.

### Native package preparation

The precompile worker now validates the raw package and precompiles the root and
every unique child into one bounded native response. The existing private-worker
nonce, full source digest, engine fingerprint and serialized digest protect the
whole response. The new trusted package decoder validates member framing and
bindings before loading the prepared components; native bytes still require the
same trusted provenance as legacy responses. Legacy components keep their existing
native encoding. The root-only decoder explicitly rejects packages so a caller
cannot silently discard isolated dependencies.

Verification: three new native codec tests cover legacy compatibility, deduplicated
roundtrip with 100 bindings, truncation/trailing bytes and corrupted child rejection.
The real package integration test now runs both root and child after worker
precompilation, rejects a wrong nonce and checks the root-only decoder rejection.
The complete component-host suite with integration/PoC features passed **76 tests**,
with one manual benchmark ignored; focused all-target Clippy passed.

The environment's package-aware prepared cache and child execution imports remain
to be connected. Native compilation stays in the worker, never in `start`.

## Remaining required work

- P0: add explicit legacy/isolated differential selection and coverage counters.
- P1: versioned execution imports, immutable package catalog, prepared child code,
  Store runner integration, aggregate guest resource reservations and teardown.
- P2: sequential/parallel Agent call backend and every AI auxiliary invocation,
  preserving package state eligibility and existing invocation semantics.
- P3: recursive Embed extraction and scoped child runtime, suspension/wake sets,
  scopes, deadlines, checkpoint keys and existing reference ABI modes.
- P4: durable attempt transitions, root and targeted command routing, crash/lease
  fencing, resource/tenant ownership and parked invocation handling.
- P5: all compatibility gates, actual baseline/candidate measurements, full unit
  and integration suites, local server plus isolated persistence E2E.
- P6: controlled opt-in and artifact-compatible rollback; no default enablement
  before all gates above pass.

No local server has been launched yet. No isolated DSL backend result has been
measured. Do not treat the task-ownership tests as proof of the full implementation.
