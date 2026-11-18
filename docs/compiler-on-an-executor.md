# The compiler on an executor

Phase 1 is built: the compiler runs as a guest of `runtara-run` on an
executor and makes the bytes a compile in process makes
([what phase 1 found](#what-phase-1-found)). The engine does not place its
compiles there yet: that is phases 2 and 3.

## What is wanted

A tenant's workflow graph is compiled in a process that holds nothing: no
database, no key, no seal, and, unlike today, not the engine's own address
space either. Today the compiler runs in a Wasmtime store inside the
engine's process ([splitting-the-engine.md](splitting-the-engine.md), Phase
6), so a flaw in Wasmtime itself would put a tenant's graph in the engine.
An executor ([executor.md](executor.md)) is where the same flaw lands in a
process with no file, no socket and no credential, on a host with no
database.

## What there is

This was traced against the code before it was written. The short of it:
almost all of this exists, for another host, and what is missing is small.

**Already in the repository**

- **A compiler that speaks to its host over the workflow API**
  (`crates/runtara-compile-guest/src/main.rs`): a WASI command with no
  filesystem. It reads its request (`GET /v1/run/input`), fetches each
  component it reads by SHA-256, in slices (`GET /v1/artifacts/{sha}`),
  hands back the composed component in parts (`PUT /v1/artifacts/{sha}`)
  and posts what it made or why it failed (`POST /v1/run/outcome`). It was
  built for a VM that grants a guest nothing else.
- **The host's side of that** (`runtara-component-host`,
  `ApiLaunches::begin_compile`): a launch whose input is the compile
  request, which is served its run, its input, its outcome and the
  components it was told it may fetch, and nothing else of the API: no
  connection, no HTTP, no SQL, no control. `ApiLaunches::made` takes what
  it handed back.
- **Where a compile is done is already a choice**
  (`compile_executor::CompileExecutor`): the hook the in-process sandbox is
  installed through. What is handed to it and what it answers is what a
  compile in this process is.
- **Everything an executor does for a run**: it fetches what it runs by its
  digest with the launch's credential, compiles it to machine code in a
  sandboxed process, starts `runtara-run`, relays the guest's `/v1/` calls
  to the engine's run channel with the launch's credential, ends the run at
  its lifetime and reports what became of the process.
- **`runtara-run` links both `wasi:http`**, 0.2 and 0.3
  (`runtara-run/src/guest.rs`). This was read as "a run on an executor is
  given both", which it was not: 0.2 was linked so as to be denied, and the
  entry a guest runs by had to be WASI 0.3's. Phase 1 changed both, for a
  WASI 0.2 command only (below).

**Missing**

- Nothing places a compile launch with an executor. The engine places
  *workflow runs* (an instance, an image, a lease); a compile is none of
  those.
- The HTTP-speaking guest is not in the bundle. Only the sandboxed one
  (`runtara-compile-sandboxed.wasm`) is built and shipped.
- An executor is asked for "the launch's workflow" by digest, from the
  artifact store (`LaunchWorkflows`). The compiler is not in that store.

## The design

A compile is a launch like a run, of a component that happens to be the
compiler.

1. **The engine begins a compile launch** (`begin_compile`): the request,
   and the files it may fetch, each by its SHA-256. These are exactly what
   the in-process sandbox is shown today: the components directory and the
   tenant's published agents.
2. **It places the launch with an executor**, as it places a run: the
   compiler's digest, the guest's limits (3 GiB of memory and five minutes,
   as the sandbox has), the run channel's endpoint and a credential minted
   for this launch.
3. **The executor fetches the compiler if it does not hold it**, with that
   credential, checks it against the digest, and compiles it to machine
   code once. It starts `runtara-run`, sandboxed, with nothing but one
   socket to its executor.
4. **The compiler compiles**, asking for its request and each component
   over the socket. The executor relays each call to the engine as that
   launch. The engine answers what a compile launch is served and refuses
   everything else.
5. **The engine takes what it handed back and trusts none of it**, exactly
   as it does with the sandbox today: the component against the digest the
   outcome names, the agents the build says it composed against the ones
   the engine finds in the graph, the trusted pins against what is
   installed, the memory floor against the version's setting. This code is
   shared with the sandbox path, not written again.
6. **The launch ends.** The executor reports what became of its process. A
   compiler that ended without an outcome (a trap, a kill, its lifetime) is
   the compile's failure.

Publishing a workflow as an agent compiles the same way
(`compile_published_agent`): it is the same request with another outcome.

### What the engine installs, and when

`RUNTARA_COMPILER_WASM` names the compiler, as now. Where it runs follows
the runner:

| Runner | A compile runs |
|---|---|
| executors (`RUNTARA_RUNNER=executor`) | on an executor |
| embedded | in the engine's sandbox, as today |

With executors, there is no falling back to the engine's own sandbox: a
compile that cannot be placed waits its turn in the compile queue and fails
if no executor takes it, as a run does (decision 1). Otherwise the property
is only as strong as the worst moment.

The in-process sandbox stays for a deployment with no executors (one
process, the evaluation stack). It is the same compiler, and removing it
would leave such a deployment compiling natively.

### What changes in each crate

- **`runtara-compile-guest`** — nothing in its code. It is built and
  shipped: `scripts/build-agent-components.sh` builds it (before the
  stdlib, as the sandboxed one is), and the bundle carries it.
  `RUNTARA_COMPILER_WASM` names the sandboxed one; the HTTP one sits beside
  it under a fixed name.
- **`runtara-environment`, `runtara-runner-executor`** — placing a launch
  that is not an instance (`Runner::compile_elsewhere`, and the executor
  provider's spec for a guest that is not a run). Done, phase 1. The
  compiler is put in the artifact store and fetched from it by digest.
- **`runtara-server`** — a `CompileExecutor` that does steps 1, 2, 5 and 6,
  installed when the runner is executors. `compile_sandbox.rs`'s checks
  move to where both use them.
- **`runtara-run`** — runs a WASI 0.2 command as well as a 0.3 one, and
  gives it `wasi:http` 0.2 to the workflow API origin (done, phase 1).
- **`runtara-executor`** — nothing.
- **`runtara-component-host`** — nothing.

### What it costs

- **A slot.** A compile takes one of an executor's run slots for as long
  as it runs (seconds, usually). Compiles and runs then compete for the
  same room.
- **The first compile at an executor** pays for compiling the compiler
  itself to machine code, once: an 8.6 MB component, some seconds. After
  that it is read from disk.
- **Round trips.** Each component is fetched over the relay instead of
  read from a file, in slices. A compile reads the stdlib and the agents
  its graph names, a few megabytes in all.
- **Progress.** The stages a compile reports from inside are not reported
  from the sandbox today, and would not be from an executor either. The
  stages around it are.

A compiler kept warm for a tenant, which takes one compile after another
without starting again (`begin_compile_worker`, also already in the
repository), removes the start-up cost. It is a later step: it holds a slot
while idle, and the cold path has to exist first.

## What a broken compile is worth, then

As in Phase 6, a graph that breaks the compiler runs code in a guest with
no credential and no address, and can at most return a wrong program for
the version it was asked to compile. What changes is one step further out:
a graph that also breaks *Wasmtime* is in `runtara-run`, which can open no
file, make no socket and signal no process, on an executor's host. Today it
would be in the engine.

What does not change: the engine still reads the graph itself, to validate
it when it is saved (in the validation sandbox, which is in the engine's
process) and to find the agents it calls. Moving validation is not part of
this plan.

## What phase 1 found

The three things that had been read around and not run.

1. **An executor did not accept the compiler as something to run.**
   `runtara-run` ran only a guest that exports `wasi:cli/run@0.3.0`, and
   gave a guest `wasi:http` 0.3 to the workflow API while denying it 0.2.
   The compiler is a WASI 0.2 command (what a Rust program built for
   `wasm32-wasip2` is) and speaks `wasi:http` 0.2. So `runtara-run` now
   also runs a guest whose entry is `wasi:cli/run@0.2.N`, which has the
   same signature, and gives *that* guest `wasi:http` 0.2 to the workflow
   API origin, over the same socket. A workflow, which runs by 0.3, is
   denied 0.2 as before. No guest is given two ways out, and neither is
   given any origin but the one.

   The other way was to port the compiler to WASI 0.3. That is a change to
   the compiler's own I/O and to how it is built, for a guest another host
   also runs; the change to `runtara-run` is twenty lines and takes nothing
   away from what a run is refused.
2. **The relay's limits fit.** A compile of a workflow with one agent
   fetched eight files (6.5 MB) and handed back 6.6 MB through
   `runtara-run` and the executor, in 0.3 s in all.
3. **A launch can be placed without an instance.** `begin_compile`, the
   run channel's credential, the launch's artifact and the provider's
   placement do not need one. `Runner::compile_elsewhere` is that
   (`runtara-environment`, `runner/provider.rs`): the engine's runner places
   the compiler, waits for what it posts, and takes what it handed back.
   The compiler is fetched from the artifact store by its digest, like a
   workflow, so the store holds it and nothing was added to
   `LaunchWorkflows`.

Not found out here: the compiler under the *real* sandbox. The tests run
`runtara-run` sandboxed on Linux, which CI is, and unsandboxed on a
developer's macOS. If the compiler makes a system call a run does not, CI
is where it shows.

## Phases

1. **The compiler under `runtara-run`, with no engine.** Done. Build and ship the
   HTTP guest. A test in `runtara-executor` (with the other
   component-integration tests) that runs it sandboxed against
   `ApiLaunches::begin_compile` and gets the bytes a compile in process
   makes. This settles the three unknowns above.
2. **The engine places compiles.** The placement entry, the
   `CompileExecutor`, the shared checks, and the rule that an engine with
   executors compiles nowhere else. Unit tests against a provider that is
   stood in for: a compile that is placed, one that no executor takes, one
   whose compiler ends without an outcome, one whose component is not the
   one named.
3. **Publishing an agent, end to end, and the documents.** `compile_published_agent`
   on the same path. `e2e/test_executor_runner.sh` and
   `e2e/test_two_roles.sh` compile through an executor and hold that the
   engine's process never loaded the compiler; `docker-compose.executor.yml`
   and the roles overlay carry the guest. `docs/executor.md`, `roles.md`,
   `splitting-the-engine.md`.

Later, not started with these: a warm compiler for each tenant.

## Decisions

Made.

1. **With executors, no falling back to the engine's sandbox.** A compile
   no executor takes fails.
2. **Compiles share the run slots.** A reserve for compiles is one setting
   to add when it is seen to matter.
3. **The compiler's machine code is kept once for an executor, not once
   for each tenant.** It is the platform's and the same for all. Each
   compile still runs in a process of its own.
4. **The engine's sandbox stays for a deployment with no executors.**
5. **A warm compiler is later.**

## Size

Phase 1 took a day. Phases 2 and 3 are two to three more: the placement
entry, which was the uncertain part, is built.
