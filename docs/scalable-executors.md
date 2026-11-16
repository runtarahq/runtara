# Scalable executors (plan)

A plan, not yet implemented. `docs/executor.md` describes what exists: one
executor per server, named by `RUNTARA_EXECUTOR_URL`, with each workflow
pushed to it by the server before its first run.

This plan lets a server use any number of executors, with no list of them
anywhere:

- an executor **registers itself** with the server and keeps a heartbeat; the
  server places runs with the executors it has heard from;
- an executor **pulls a workflow** it does not hold from the server that
  placed the run, instead of being sent it beforehand.

What stays as it is: both directions are mutual TLS between the deployment's
two certificates; an executor holds no database connection and no credential
beyond its certificate; `runtara-run` and its sandbox are untouched; the
server still calls the executor to place, watch and stop a run.

```text
                 register, heartbeat, fetch workflow, runs' API calls
runtara-executor ───────────────────────────────────────────────▶ runtara-server
runtara-executor ◀─────────────────────────────────────────────── (run channel listener)
                 place, state, stop
```

## Why

- **One executor is the ceiling.** The server takes one URL. Replicas behind
  one address do not work: each executor keeps its runs in memory and its
  compiled workflows on its own disk, so a run's status call that reaches
  another replica is answered "no such run" and the run is recovered as lost.
- **Pushed workflows go missing.** The server remembers for ten minutes that a
  workflow is staged (`STAGED_FOR` in `runtara-environment`'s
  `runner/provider.rs`). An executor that loses its cache in that time refuses
  every run of that workflow as not staged until the server forgets. With
  several executors the server would have to track this per executor.

## Order of work

1. Executors pull workflows. Stands alone with one executor, fixes the gap
   above, and is what makes a second executor cheap: the server no longer
   needs to know what each executor holds.
2. Registration and heartbeat; placement over the registered executors.
3. Compose, documentation, end-to-end test.

Later, not in this plan: executors that also pull their work (see the end).

## 1. Executors pull workflows

### The server

- A new route on the run channel's listener, beside the workflow API
  (`runtara-runner-executor`'s `prepare`):

  `GET /executor/v1/artifacts/{sha256}`

  Authorized by a launch's bearer credential, it answers with the workflow
  component of that launch and of no other: the digest in the path must be the
  one the launch was placed with. The tenant is the launch's.
- It is served before the launch's start is confirmed and does not count as
  the run's first call. `RunCredentials` (`runtara-component-host`'s
  `run_channel.rs`) gets a lookup that neither waits at the start gate nor
  sets `first_used`. Fetching a workflow is not the guest acting, so the gate
  has nothing to hold back.
- The path is outside `/v1/` on purpose: an executor relays only `/v1/` from a
  guest, so a guest cannot ask for it.
- `ExecutorProvider::stage` no longer calls the executor: it returns the
  digest. The `Provider` trait keeps `stage`; other providers may still push.
- `ProviderRunner` records each launch's digest while the launch is served, for
  the route to check against. The route reads the bytes from the artifact
  store the runner is given; `prepare` runs before the runner exists, so the
  two share a handle that the runner's factory fills.

### The executor

- `Runs::place` checks the spec and takes a run slot as now, so a full
  executor still refuses at once. It then returns the run's id without
  waiting for the workflow.
- The run's supervising task, when the workflow is not held for the tenant:
  fetches it from the run's channel endpoint with the launch's credential,
  bounded by `MAX_ARTIFACT_BYTES`; hands it to `Artifacts::put`, which already
  checks the bytes against the digest and compiles one workflow once however
  many runs wait for it; then starts the process.
- A fetch that fails or a compile that is refused ends the run as `Failed`
  and posts the run's failed outcome, as a start that fails does now.
- The placement API loses `GET` and `PUT /v1/artifacts/{tenant}/{sha256}` and
  the 128 MB body limit they needed.

A run is `Running` to the server while its workflow is fetched and compiled,
so that time is inside the server's timeout for the run. The executor's
lifetime for the guest still starts with the process.

## 2. Registration and heartbeat

### The protocol

On the run channel's listener, which already accepts only the executor's
certificate. One call both registers and renews:

`PUT /executor/v1/executors/{boot}`

```json
{ "address": "https://10.0.0.7:7443", "max_runs": 16, "running": 3, "draining": false }
```

- `boot` is an id the executor makes each time it starts.
- `address` is where the server places runs with it. Left out, the server
  uses the address the call came from and the port in `port` (the executor's
  listening port): what a container on a private network needs. Set
  (`RUNTARA_EXECUTOR_ADVERTISE_URL`) when the executor is reached through
  something else.
- The answer says how often to call again and after how long without a call
  the executor counts as away, so both are the server's to set.

`DELETE /executor/v1/executors/{boot}` when an executor stops.

Because each call carries everything, there is no separate "register again":
a server that started again learns its executors from their next call.

The address is not trusted for being sent: the server connects to it
requiring the `runtara-executor` certificate, as it does now, so an address
that is not an executor of this deployment fails the handshake.

### Timing

| | |
|---|---|
| Heartbeat | every 5 seconds |
| Away | no heartbeat for 15 seconds: no new runs are placed there |
| A run lost | as now: its executor unreachable for 30 seconds, or answering that it does not hold the run |

Away only stops placements. A run is still lost by its own status call, so a
late heartbeat costs no healthy run.

### The executor

- `RUNTARA_EXECUTOR_SERVER_URL` (required): the server's run channel.
  `RUNTARA_EXECUTOR_ADVERTISE_URL` (optional).
- A task that calls the server at start and at the interval it was told,
  with `running` from `Runs::running`. It keeps trying while the server is
  away; runs it holds are unaffected.
- On `SIGTERM`: `draining: true` at once, so nothing new is placed while its
  runs end, then the `DELETE`.

### The server

- `RUNTARA_EXECUTOR_URL` is removed. `RUNTARA_RUN_CHANNEL_ENDPOINT` stays: it
  is what a run's spec names.
- A registry in memory: per `boot`, its address, a TLS client, capacity, what
  it last reported, and when. `ExecutorProvider` places from it.
- **Choosing**: among executors that are not away and not draining, the one
  with the most free slots (what it reported, less what this server has
  placed there since), turns taken on a tie.
- **Failing over**: an executor that refuses for capacity or cannot be reached
  is passed over for a moment and the next is tried. A run no executor takes
  is refused as now and offered again by recovery.
- **Not waiting on a full executor**: `Place` gets an optional `slot_wait_ms`.
  The server sends 0 while it has another executor to try, and leaves the
  5 seconds for the last.
- **A run's executor**: the provider's run id becomes `<boot>:<run>`, so a
  status or stop call goes to the executor that holds the run. It is kept in
  memory only, as the run id is now.
- **A restarted executor**: a new `boot` from an address means the runs of the
  old one there are gone. They are reported lost then, without waiting for
  their status calls.
- **No executor registered**: at start, or when all are away, a placement is
  refused and offered again. A server with `RUNTARA_RUNNER=executor` still
  never runs a workflow itself.

### Certificates

Unchanged: one server certificate and one executor certificate
(`runtara-executor init-pki`). Every executor uses the executor's, so
replicas started from one definition share its directory read-only.

## 3. Compose, documentation, tests

- `docker-compose.executor.yml`: the executor is told the server's run
  channel; the server is told no executor. More executors are
  `docker compose ... up --scale executor=3`: each replica has its own
  address on the private network and its own cache (`executor_data` becomes
  an anonymous volume per replica).
- `docs/executor.md`: the settings, "How a run goes", and "One executor per
  server" are rewritten; this file is removed once it says nothing more.
- `runtara-executor`'s `protocol.rs` table, and the `HEAD` it names for a
  route that was a `GET`.

Tests, with the feature flags `ci.yml` already sets:

- `runtara-component-host`: the workflow lookup answers before the start gate
  opens, is refused once the credential is revoked, and does not set the
  run's start.
- `runtara-executor`: a workflow not held is fetched with the launch's
  credential; bytes that are not the digest's are refused; runs placed at once
  fetch and compile once; one tenant's workflow is not another's; an executor
  registers, renews, reports draining and removes itself.
- `runtara-runner-executor`: runs spread over two executors; a full one is
  passed over; one that stops loses its runs and no others; one that starts
  again with an empty cache runs the workflow; a server that starts again
  places runs after the next heartbeat; no executor means a refused, not a
  failed, run.
- `e2e/test_executor_runner.sh`: two executors, both of which took runs.

## Later: executors that pull their work

With registration the server still connects to each executor. The step after
is for an executor to take its runs over its own connection to the server,
and report them the same way: nothing then connects to an executor, no
address is advertised, and an executor can sit behind anything that lets it
call out. It replaces the placement API and how the server watches a run, so
it is its own piece of work.
