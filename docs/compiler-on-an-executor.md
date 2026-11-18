# The compiler on an executor

A plan. Nothing here is built.

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
- **`runtara-run` links both `wasi:http`**: 0.2 and 0.3
  (`runtara-run/src/guest.rs`). Phase 6 did not use the existing guest
  because the engine's *embedded* runner serves the workflow API to 0.3
  only. A run on an executor has no such limit, so the reason Phase 6 gave
  for not placing compiles on executors does not hold there.

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
if no executor takes it, as a run does. Otherwise the property is only as
strong as the worst moment.

The in-process sandbox stays for a deployment with no executors (one
process, the evaluation stack). It is the same compiler, and removing it
would leave such a deployment compiling natively.

### What changes in each crate

- **`runtara-compile-guest`** — nothing in its code. It is built and
  shipped: `scripts/build-agent-components.sh` builds it (before the
  stdlib, as the sandboxed one is), and the bundle carries it.
  `RUNTARA_COMPILER_WASM` names the sandboxed one; the HTTP one sits beside
  it under a fixed name.
- **`runtara-runner-executor`** — an entry for placing a launch that is not
  an instance: the placement, its status and its stop, without the
  instance's row and lease. And `LaunchWorkflows` answers a compile
  launch's artifact with the compiler's bytes, which the engine holds in
  memory from boot, rather than from the artifact store.
- **`runtara-server`** — a `CompileExecutor` that does steps 1, 2, 5 and 6,
  installed when the runner is executors. `compile_sandbox.rs`'s checks
  move to where both use them.
- **`runtara-executor`, `runtara-run`** — nothing, if the two things under
  "To find out first" hold.
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

## To find out first

Three things I have read around but not run. Each is an afternoon, and each
could change the plan, so they are phase 1.

1. **Whether an executor accepts the compiler as something to run.**
   `runtara-run compile` refuses a component that imports what a run is not
   given. The compiler imports WASI and `wasi:http` 0.2, which a run is
   given, but I have not run it through.
2. **Whether the relay's limits fit.** The guest fetches in slices of up to
   8 MiB and hands back in parts sized for the VM it was built for (4 MiB).
   `runtara-run` and the executor each cap a relayed body; I have not
   compared the numbers.
3. **Whether a launch can be placed without an instance.** The provider
   builds the placement from an instance's launch. If the launch's protocol
   fields assume a lease and an instance row, the entry in
   `runtara-runner-executor` is more than a thin one.

## Phases

1. **The compiler under `runtara-run`, with no engine.** Build and ship the
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

1. **With executors, no falling back to the engine's sandbox.** A compile
   no executor takes fails. (Recommended: yes.)
2. **Compiles share the run slots.** The alternative is a number of slots
   an executor keeps for compiles, so that a burst of runs cannot hold up
   every save. (Recommended: share them now; a reserve is one setting to
   add when it is seen to matter.)
3. **The compiler's machine code is kept once for an executor, not once
   for each tenant.** Workflows are kept per tenant because a workflow is a
   tenant's. The compiler is the platform's and the same for all; keeping
   it per tenant compiles 8.6 MB again for each tenant's first save. Each
   compile still runs in a process of its own. (Recommended: once.)
4. **The engine's sandbox stays for a deployment with no executors.**
   (Recommended: yes.)
5. **A warm compiler is later.** (Recommended: yes.)

## Size

Three to five days if the three unknowns hold: most of the machinery
exists. If a launch cannot be placed without an instance (the third), add
two days for the placement entry.
