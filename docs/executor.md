# Workflows on an executor

By default `runtara-server` runs every workflow itself, under Wasmtime, in the
process that also serves the API and holds the keys. With executors, no
workflow runs in the server: each run is a WebAssembly guest in a sandboxed
process of its own, on a host (a container, another machine) that holds no
database connection and none of the server's secrets.

```text
runtara-server ◀──mutual TLS──▶ runtara-executor ──socket pair──▶ runtara-run
 places runs                     one supervisor per run            one process per run:
 serves the run channel          holds the launch's credential     the guest, sandboxed,
                                 relays the guest's API calls      no network, no files
```

A server has as many executors as have told it they are here. It is
configured with none of them.

- **`runtara-server`** compiles a workflow as before and places each run with
  an executor. It serves the *run channel*: where executors say they are
  here, where they fetch the workflows they run, and the workflow API of the
  runs it placed, each call carrying the credential minted for its launch.
- **`runtara-executor`** is a service the server places runs with. It tells
  the server it is here and keeps telling it. For each run it fetches the
  workflow if it does not hold it, starts a `runtara-run`, relays the guest's
  workflow API calls to the server's run channel (adding the launch's
  credential, which the guest's process never sees), ends the run at its
  lifetime, and reports what became of its process.
- **`runtara-run`** is one run. It sandboxes itself before it reads anything,
  loads the compiled workflow from a descriptor it was handed, and gives the
  guest one way out: `wasi:http` to the workflow API origin, over a socket
  pair to its executor.

All three are in the release bundle's `bin/` and in the `runtara` image.

## What the sandbox is

`runtara-run` applies it to itself at start, with one thread and before it
parses its spec or the workflow:

- no core dump, a small descriptor table, `no_new_privs`, every capability
  dropped, not dumpable, every descriptor it was not handed closed;
- **Landlock**: every filesystem and network right handled and none allowed;
  signals and abstract Unix sockets scoped to the process where the kernel
  can (Linux 6.12);
- **seccomp**: a list of the system calls a Wasmtime guest and its relay
  need (memory, futexes, the event loop, clocks, I/O on descriptors it already
  holds, threads). Everything else fails: no file is opened, no socket made,
  no program executed, no other process signalled.

The guest gets what a workflow gets on the embedded runner and nothing more:
WASI without files, sockets or DNS; `wasi:http` to the workflow API origin
only; memory bounded by its launch's limit; a lifetime. Agents' outbound
requests, connections and credentials stay where they were: behind the
workflow API, in the server (and seal).

The executor checks the sandbox at start (`runtara-run selftest`, which
applies it and tries what it must refuse) and does not start on a host where
it does not hold.

### What it does not do

- **The kernel is shared.** A guest that escapes Wasmtime *and* has a kernel
  exploit reaches the executor's host. This is defence in depth for a
  self-hosted deployment, not the boundary a virtual machine is.
- **No cgroup limits and no separate user yet.** A run's memory is bounded by
  Wasmtime and its time by the executor and the server; CPU is not shared out
  between runs, and runs have the executor's uid.
- **Linux only.** Elsewhere `runtara-run` refuses to start, unless the
  executor is told `RUNTARA_EXECUTOR_UNSANDBOXED=true`, which runs guests in
  plain processes: for development, never for a deployment.

## Setting it up

### Certificates

The server and its executors authenticate each other with certificates of one
CA, made once:

```sh
runtara-executor init-pki --server-dir <dir for the server> --executor-dir <dir for the executor>
```

Each directory is given `ca.pem`, `tls.pem` and `tls.key` (its own key, mode
`600`). The CA's key is written nowhere, so nothing else can be signed by it;
to replace a certificate, run it again with `--replace` and restart both
sides. The certificates are good for five years.

Each side accepts only the other's certificate, by the name in it
(`runtara-server`, `runtara-executor`): the names are not host names, so the
hosts can be called anything. Every executor uses the executor's directory:
copy it to each, or mount it read-only.

### An executor

| Setting | |
|---|---|
| `RUNTARA_EXECUTOR_SERVER_URL` | `https://host:port` of the server's run channel (required): where it says it is here |
| `RUNTARA_EXECUTOR_TLS_DIR` | the executor's directory from `init-pki` (required) |
| `RUNTARA_EXECUTOR_STATE_DIR` | where compiled workflows are kept (required); a cache, safe to lose, and not shared between executors |
| `RUNTARA_EXECUTOR_LISTEN` | where the server connects; default `0.0.0.0:7443` |
| `RUNTARA_EXECUTOR_ADVERTISE_URL` | `https://host:port` at which the server reaches it, when that is not the address its calls come from with the port it listens on (it is behind a proxy or address translation) |
| `RUNTARA_EXECUTOR_MAX_RUNS` | runs at once; default `16`. A run over it waits up to 5 seconds for a slot, then is refused for the server to offer again |
| `RUNTARA_EXECUTOR_RUN_BIN` | the `runtara-run` to start; default: the one beside `runtara-executor` |
| `RUNTARA_EXECUTOR_NO_LANDLOCK` | `true` runs with seccomp alone on a kernel without Landlock (before 5.13) |

It needs no database, no Valkey and no credential of any kind beyond its
certificate. It must be able to reach the server's run channel, and the
server must be able to reach it.

### The server

| Setting | |
|---|---|
| `RUNTARA_RUNNER` | `executor` (default `embedded`: workflows in the server's process) |
| `RUNTARA_EXECUTOR_TLS_DIR` | the server's directory from `init-pki` |
| `RUNTARA_RUN_CHANNEL_LISTEN` | where the run channel listens; default `0.0.0.0:7444` |
| `RUNTARA_RUN_CHANNEL_ENDPOINT` | `https://host:port` at which executors reach it |
| `RUNTARA_EXECUTOR_RUNNER_LIMIT` | runs being placed at once; default `16` |

The run channel is its own listener, apart from the public API: mutual TLS,
for executors only. Keep it on the private network between them.

A server told `RUNTARA_RUNNER=executor` that cannot honour it (no
certificate, a port it cannot bind) does not start. It never falls back to
running workflows in its own process: with no executor, runs wait.

### Executors come and go

An executor tells the server it is here when it starts and every 5 seconds
after, with how many runs it holds. Nothing else adds one: start another with
the same settings and the server uses it; start the server again and it
learns its executors from their next call.

- A run goes to the executor with the most room. One that is full, or cannot
  be reached, is passed over for the next.
- An executor not heard from for 15 seconds is given no new run. The runs it
  holds are not lost by that: a run is lost when its own executor has not
  answered about it for 30 seconds.
- An executor that is stopped (`SIGTERM`) tells the server, which recovers
  the runs it held at once. So does one that starts again at the same
  address: it holds none of its earlier runs.
- A run that no executor takes is not failed. It is offered again, as a run
  the server could not place always is.

Where the server reaches an executor is what the executor said, and is not
trusted for being said: the server connects to it requiring the executor's
certificate, so an address that is not an executor of this deployment fails
the handshake.

### Docker Compose

`docker-compose.executor.yml` is an overlay on the evaluation stack that adds
an executor as a container of its own, makes the certificates on first
start, and sets the server's side:

```sh
docker compose -f docker-compose.yml -f docker-compose.executor.yml up
```

For three executors, `RUNTARA_EXECUTORS=3` before it, or `--scale executor=3`
after `up` to change it while the stack runs.

## How a run goes

1. The server compiles the workflow (with the workflow stdlib composed in:
   a run on an executor is given WASI and nothing else to link).
2. The launch is placed with the executor that has the most room: `POST
   /v1/runs` with the workflow's SHA-256, the guest's limits and the run
   protocol's fields (the launch, the run channel's endpoint and the launch's
   credential). The executor answers at once with the run's id.
3. An executor that does not hold the workflow for the tenant fetches it:
   `GET /executor/v1/artifacts/{sha256}` on the run channel's listener, with
   the launch's credential, which is answered with that launch's workflow and
   no other. It checks the bytes against the digest and compiles them to
   machine code in a sandboxed `runtara-run compile`, which refuses a
   component that does not export `wasi:cli/run` or that imports something a
   run is not given (a run is given WASI and `wasi:http`, and nothing else
   with a function in it). However many runs wait for a workflow, it is
   fetched and compiled once. Compiled workflows are kept per tenant, per
   executor. A workflow that cannot be had ends the run as failed
   (`LAUNCH_FAILED`).
4. The executor starts the process, handing it the compiled workflow and one
   end of a socket pair.
5. The guest's calls (`GET /v1/run/input`, checkpoints, `POST /v1/http`, …)
   go over the socket to the executor, which sends each on to the run
   channel as that launch. The server answers them as it does for a local
   run, fenced by the launch's lease. A guest cannot ask for a workflow: an
   executor relays only `/v1/`.
6. The guest posts its outcome and returns. If its process ends without one
   (a trap, a kill, its lifetime), the executor posts the failed outcome
   itself: `GUEST_FAILED`, `GUEST_KILLED`, `LIFETIME_EXCEEDED`, `NO_OUTCOME`
   or `LAUNCH_FAILED`.
7. The server polls `GET /v1/runs/{id}` at the run's executor for what became
   of the process, and stops a run (`DELETE`) that is cancelled or past its
   timeout.

The time a workflow takes to be fetched and compiled, on its first run at an
executor, is part of that run's timeout. The guest's lifetime starts with its
process.

An executor that cannot be reached for 30 seconds, or that left or started
again and no longer holds a run, has lost it: the server recovers the run as
it recovers any run whose runner went away. An executor's runs end with it,
and so does a run whose executor closed its socket.

## What is different from the embedded runner

These hold for any runner that places runs elsewhere (they are properties of
`runtara-environment`'s provider runner):

- **Debug mode** is off, and a run's result carries no peak memory and no
  guest output. The guest's output is in the executor's log at `debug`.

And for executors in particular:

- **One server per executor.** An executor tells one server it is here.
  Which executors a server has is kept in that server's memory.
- **The server connects to its executors**, to place a run, to ask about it
  and to stop it. An executor that only calls out, and takes its runs over
  its own connection, is not built.

## Trying it

- `e2e/test_executor_runner.sh` builds the three binaries, makes
  certificates, starts an executor and runs e2e suites against a server
  using it.
- `e2e/test_scalable_executors.sh` starts a server alone and has executors
  join, lose their cache, die and stop, and the server start again.
- `cargo test -p runtara-run -p runtara-executor -p runtara-runner-executor
  --features runtara-run/component-integration-tests,runtara-executor/component-integration-tests,runtara-runner-executor/component-integration-tests`
  after `scripts/build-agent-components.sh` and `cargo build -p runtara-run`.
