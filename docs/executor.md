# Workflows on an executor

By default `runtara-server` runs every workflow itself, under Wasmtime, in the
process that also serves the API and holds the keys. With an executor, no
workflow runs in the server: each run is a WebAssembly guest in a sandboxed
process of its own, on a host (a container, another machine) that holds no
database connection and none of the server's secrets.

```text
runtara-server ◀──mutual TLS──▶ runtara-executor ──socket pair──▶ runtara-run
 places runs                     one supervisor per run            one process per run:
 serves the run channel          holds the launch's credential     the guest, sandboxed,
                                 relays the guest's API calls      no network, no files
```

- **`runtara-server`** compiles a workflow as before, stages it with the
  executor and places each run there. It serves the *run channel*: the
  workflow API of the runs it placed, each call carrying the credential minted
  for its launch.
- **`runtara-executor`** is the service the server places runs with. For each
  run it starts a `runtara-run`, relays the guest's workflow API calls to the
  server's run channel (adding the launch's credential, which the guest's
  process never sees), ends the run at its lifetime, and reports what became
  of its process.
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

The server and the executor authenticate each other with certificates of one
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
hosts can be called anything.

### The executor

| Setting | |
|---|---|
| `RUNTARA_EXECUTOR_TLS_DIR` | its directory from `init-pki` (required) |
| `RUNTARA_EXECUTOR_STATE_DIR` | where compiled workflows are kept (required); a cache, safe to lose |
| `RUNTARA_EXECUTOR_LISTEN` | where the server connects; default `0.0.0.0:7443` |
| `RUNTARA_EXECUTOR_MAX_RUNS` | runs at once; default `16`. A run over it waits up to 5 seconds for a slot, then is refused for the server to offer again |
| `RUNTARA_EXECUTOR_RUN_BIN` | the `runtara-run` to start; default: the one beside `runtara-executor` |
| `RUNTARA_EXECUTOR_NO_LANDLOCK` | `true` runs with seccomp alone on a kernel without Landlock (before 5.13) |

It needs no database, no Valkey and no credential of any kind beyond its
certificate. It must be able to reach the server's run channel.

### The server

| Setting | |
|---|---|
| `RUNTARA_RUNNER` | `executor` (default `embedded`: workflows in the server's process) |
| `RUNTARA_EXECUTOR_URL` | `https://host:port` of the executor |
| `RUNTARA_EXECUTOR_TLS_DIR` | its directory from `init-pki` |
| `RUNTARA_RUN_CHANNEL_LISTEN` | where the run channel listens; default `0.0.0.0:7444` |
| `RUNTARA_RUN_CHANNEL_ENDPOINT` | `https://host:port` at which the executor reaches it |
| `RUNTARA_EXECUTOR_RUNNER_LIMIT` | runs being placed at once; default `16` |

The run channel is its own listener, apart from the public API: mutual TLS,
for the executor only. Keep it on the private network between the two.

A server told `RUNTARA_RUNNER=executor` that cannot honour it (no
certificate, a port it cannot bind) does not start. It never falls back to
running workflows in its own process.

### Docker Compose

`docker-compose.executor.yml` is an overlay on the evaluation stack that adds
the executor as a container of its own, makes the certificates on first
start, and sets the server's side:

```sh
docker compose -f docker-compose.yml -f docker-compose.executor.yml up
```

## How a run goes

1. The server compiles the workflow (with the workflow stdlib composed in:
   a run on an executor is given WASI and nothing else to link).
2. On the workflow's first launch it is staged: `PUT
   /v1/artifacts/{tenant}/{sha256}`. The executor checks the bytes against
   the digest and compiles them to machine code in a sandboxed `runtara-run
   compile`, which refuses a component that does not export `wasi:cli/run` or
   that imports something a run is not given (a run is given WASI and
   `wasi:http`, and nothing else with a function in it). Compiled workflows
   are kept per tenant.
3. The launch is placed: `POST /v1/runs` with the guest's limits and the run
   protocol's fields (the launch, the run channel's endpoint and the
   launch's credential). The executor starts the process, handing it the
   compiled workflow and one end of a socket pair.
4. The guest's calls (`GET /v1/run/input`, checkpoints, `POST /v1/http`, …)
   go over the socket to the executor, which sends each on to the run
   channel as that launch. The server answers them as it does for a local
   run, fenced by the launch's lease.
5. The guest posts its outcome and returns. If its process ends without one
   (a trap, a kill, its lifetime), the executor posts the failed outcome
   itself: `GUEST_FAILED`, `GUEST_KILLED`, `LIFETIME_EXCEEDED`, `NO_OUTCOME`
   or `LAUNCH_FAILED`.
6. The server polls `GET /v1/runs/{id}` for what became of the process, and
   stops a run (`DELETE`) that is cancelled or past its timeout.

An executor that cannot be reached for 30 seconds, or that started again and
no longer holds a run, has lost it: the server recovers the run as it
recovers any run whose runner went away. An executor's runs end with it, and
so does a run whose executor closed its socket.

## What is different from the embedded runner

These hold for any runner that places runs elsewhere (they are properties of
`runtara-environment`'s provider runner):

- A workflow's **isolated agent children** (agent steps under an isolation
  policy) are not available: they are child tasks of the run's own process.
  `workflow:` child runs work.
- **Debug mode** is off, and a run's result carries no peak memory and no
  guest output. The guest's output is in the executor's log at `debug`.
- The **scoped agent** review configuration is not consulted.
- One executor per server.

## Trying it

- `e2e/test_executor_runner.sh` builds the three binaries, makes
  certificates, starts an executor and runs e2e suites against a server
  using it.
- `cargo test -p runtara-run -p runtara-executor -p runtara-runner-executor
  --features runtara-run/component-integration-tests,runtara-executor/component-integration-tests,runtara-runner-executor/component-integration-tests`
  after `scripts/build-agent-components.sh` and `cargo build -p runtara-run`.
