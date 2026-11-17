# Separating the public API from the engine

A plan. Phases 0 (branch `feature/row-security-fails-closed`), 1 (branch
`feature/seal-out-of-process`, on top of it) and most of 2 (branch
`feature/engine-interface`, on top of that) are implemented; the rest is not. This file is removed when the work lands and `docs/`
describes the result instead.

## The goal

Today one process, `runtara-server`, answers every request from the internet
and also holds everything worth stealing: the keys that open connection
secrets, both databases, the run channel and the executors' certificates.
Whoever breaks that process has all of it, for every tenant it serves.

After this work, a deployment can run three processes instead:

```text
internet ──▶ API role ──mutual TLS──▶ engine role ──▶ seal
             parses requests           durable state      the only holder of
             CRUD on definitions       compiles, places   the private keys;
             MCP, webhook intake       runs, run channel  sends credentialed
             no keys, no runtime DB    no public listener requests itself
```

The rule the split is built on:

> **The API role holds no authority of its own.** Everything the engine does
> for it is authorised by something the outside caller supplied (a user's
> token, an API key, a webhook's signature, a trigger's unguessable id) and
> the engine checks that itself. Breaking the API role gives an attacker
> nothing an outside caller with the same inputs could not already do, plus
> the traffic that passes through it.

A single process that does everything stays the default, as `embedded` stays
the default runner: one binary to evaluate, the split when isolation matters.

## Where things are today

From a survey of the code on `main` plus `feature/scalable-executors`. File
references are where to start reading, to be confirmed as each phase begins.

- **Two databases already.** The server database
  (`RUNTARA_SERVER_DATABASE_URL`, `crates/runtara-server/migrations`) holds
  definitions: workflows, connections (sealed), triggers, API keys, the
  execution outbox and admission tables. The runtime database
  (`RUNTARA_DATABASE_URL`, `runtara-store-postgres` and
  `runtara-environment` migrations) holds instances, checkpoints, signals,
  waits and images.
- **Handlers never query the runtime database.** They reach it through two
  concrete structs: `RuntimeClient` (`src/runtime_client.rs`, about 40
  methods over an in-process `EnvironmentHandlerState`) and `ExecutionEngine`
  (`src/workers/execution_engine.rs`), which mixes the outbox and admission
  with runtime reads and commands. There is no trait between handlers and
  the engine.
- **Launching is already a queue.** A launch writes the outbox in the server
  database, a relay publishes it to a Valkey stream, and only the trigger
  worker calls `start_instance`.
- **The seam is about 25 to 30 operations**, from about 50 to 60 call sites
  in handlers and channels (listed under Phase 2).
- **Compilation runs in the server process**, from a Valkey queue consumed by
  a worker in the same process, or directly when there is no Valkey.
- **seal runs in the server process** (`runtara-connections`
  `sealed/mod.rs`, `seal_client::in_process`). Tenants' private keys are in
  the server database (`seal_wrapped_key`), wrapped by a key the process
  reads from its environment. The server can open every credential.
- **Every credentialed call is made by the server**, in both runner modes: a
  guest's HTTP, SQL and presign calls arrive on the run channel and the
  server has seal send them. Executors hold no secrets.
- **Handlers use seal directly** for: saving a connection, the OAuth code
  exchange and token revocation, resolving a connection's resources,
  checking and registering webhook secrets, chat channel replies, the object
  model's Postgres sessions, and agent capability tests.
- **Tenant scoping is in SQL and in row-level security.** Since Phase 0 a
  connection names a tenant, or asks for every tenant as the platform, or is
  refused; the process still chooses which, until Phase 4 gives the API role
  a database role that may not ask for every tenant.
- **MCP tools call the HTTP handlers in-process** (`Router::oneshot` on an
  internal router), so MCP follows wherever the handlers go.

## What each role holds when this is done

| | API role | Engine role | seal |
|---|---|---|---|
| Public listener | yes | no | no |
| Server database | a role limited to definition tables | yes | no |
| Runtime database | no | yes | no |
| Keys that open secrets | no | no | yes |
| May ask seal to use a secret | no | yes | — |
| Tenants' public keys (to seal new secrets) | yes | yes | — |
| Run channel, executor certificates | no | yes | no |
| Compiles and runs workflow or agent code | no | yes | no |
| Artifact store credentials | no | yes | no |
| Valkey | sessions, quotas, membership, progress | streams and queues | no |
| Verifies end-user tokens and API keys | yes, to refuse early | yes, to decide | no |

## Phase 0: done

Independent of the split. Implemented on `feature/row-security-fails-closed`,
with one bug found and fixed on the way.

### Row-level security that fails closed

A connection with no tenant set saw, and could write, every tenant's rows.
Now:

- **Two scopes, each entered by name** (`runtara-tenant-scope`, a crate with
  no database, so any crate can keep the scope of work it spawns):
  `scoped(tenant, work)` and `system(work)`. The pool names the scope on
  each connection (`runtara.tenant_id`, `runtara.all_tenants`).
- **No scope is refused.** A forward migration in each database rewrites
  every policy: the tenant's rows by the column, or every tenant's when the
  session asked and may, and otherwise `runtara: no tenant scope`, for reads
  and writes. It costs what the old policy did.
- **Who may ask for every tenant is the database's to say:** any role until
  a role named `runtara_system` exists, then only its members.
- **Every path says what it is.** Start-up, maintenance commands and the
  workers they start are the platform's; spawned work keeps the scope of
  what spawned it; a request starts in none, and an unauthenticated one has
  its tenant found as the platform (`locating`) and then works as that
  tenant.
- `RUNTARA_ROW_SECURITY_UNSCOPED=system` gives work in no scope every
  tenant's rows with a warning that says where it ran, for finding such a
  path in a deployment before refusing it.

What it left as it was, and what follows from it:

- **A run's own database work is the platform's**, as it was the system
  path before. Narrowing it to the run's tenant is open; the engine side of
  Phase 2 is where a run's tenant is at hand.
- **`RUNTARA_REQUIRE_ROW_SECURITY` is still opt-in.** On by default it
  would refuse the superuser the evaluation stack connects as.
- **The upgrade is not handover-safe:** an older release still running on
  the migrated databases as a plain role has its workers refused until it
  is stopped.
- **CI does not catch a missing scope**, because its databases connect as a
  superuser. `e2e/test_served_tenants.sh` and
  `e2e/test_scalable_executors.sh` run the server as a plain role and do;
  neither is in CI.
- **A host that embeds the server** and reads policed tables from tasks of
  its own gives that work a scope too.

### API keys revoked at every start (found on the way, fixed)

Not row-level security, but found while running the server as a plain role.
At start the server revokes the API keys of users who are no longer members
of their tenant, and it took a missing membership record in Valkey for that.
Nothing writes those records in `local` and `trust_proxy` auth, and under
`oidc` the gate every request passes ignores them unless
`RUNTARA_AUTH_MEMBERSHIP_POLICY=required`, which is not the default. So
each start revoked every key a user had made.

The sweep now follows the gate: it revokes under `oidc` with `required`,
says what it would revoke under `logging`, and otherwise does nothing.

What it means for the split: the sweep and the gate are two readers of one
rule, and they had drifted apart. In Phase 4 the engine is the one that
verifies a key, so both belong on the engine side, reading the rule from
one place.

### The synchronous HTTP trigger (accepted as it is)

`events/http-sync/{workflow_id}` runs a workflow for whoever knows its id.
That is accepted: the id is the secret, as a trigger's id is for the
asynchronous route. In Phase 4 the engine holds it to exactly that and no
more.

### Listeners say what they are

`docs/executor.md` now says the run channel (7444) and executor (7443)
listeners are for the deployment's network only, and that mutual TLS is the
last barrier, not the only one.

## Phase 1: done

seal runs as a process of its own when the deployment says so. Implemented
on `feature/seal-out-of-process`; `docs/seal.md` describes the result.

- **`RUNTARA_SEAL_MODE`** is `in_process` (the default), `unix` or `mtls`.
  The server builds a client for seal's socket or address instead of seal
  itself, and every keyset operation (make one, read its public keys,
  destroy it) is one of seal's calls.
- **The server holds no wrapping key** beside a seal of its own, and is
  refused one. It does not start if seal does not answer, or does not know
  it.
- **The keys are handed over, not copied by hand.** `seal-keys export`
  writes the database's keys in seal's directory layout, with seal's own
  code, and the key that wraps them as seal reads a wrapping key file.
  A server does not start on a seal that lacks a key its database still
  holds, so a switch of mode cannot orphan what is already sealed.
  `seal-keys forget` deletes the database's copy once seal answers for
  every key.
- **Re-wrapping is seal's.** Runtara's own loop for wrapping tenants' keys
  again after an encryption key rotation is gone; the database key source
  lists and replaces its keys, and seal's `rewrap` does the rest.
- **seal is at its current revision**, which changed what a policy holds:
  where a credential may go, and no longer what it may do there.
- **The evaluation stack** has `docker-compose.seal.yml`: seal on a socket
  the two containers share.

Tested against seal's own service inside the crate's tests, and end to end
(`e2e/test_seal_process.sh`): one connection sealed in the server keeps
working as the keys are handed over, on a socket, over mutual TLS, and
until its tenant's keys are destroyed.

What it left, and what follows from it:

- **What a separate seal may reach is its own configuration.** In the
  server's process, seal's network allowances followed the server's
  settings. A separate seal is told itself (`[network]`), and the two can
  disagree: the server seals a secret for a private address that seal then
  refuses to go to. `docs/seal.md` says so; nothing checks it at start.
- **Audit and limits are seal's**, as planned: the server no longer writes
  seal's audit records, and seal's own caps apply.
- **The server is one caller, for every tenant.** seal knows it as
  `runtara-server` with every keyset. In Phase 3 the engine is that caller
  and the API role is none, which is where this starts to matter.
- **The hosted setup** (several seal instances on a bucket, the wrapping
  key in a KMS) is seal's own configuration and needs nothing more from
  Runtara; it was not run here.

## Phase 2: done, but for what is listed last

One interface between the code that takes requests and the engine, in one
process, with no change in behaviour. Implemented on
`feature/engine-interface`.

- **`Engine`** (`crates/runtara-server/src/engine.rs`) is the whole of what
  the API asks of the engine, and `LocalEngine` answers it by calling the
  engine in this process. It is a module of the server and not a crate of
  its own: both roles are one binary, and the types it speaks are the
  server's.
- **Operations**, each naming the tenant it is for:

  | Group | Operations |
  |---|---|
  | Launch | start a run, run and wait, accept a trigger's event, replay |
  | Lifecycle | stop, pause, resume |
  | Reads | runs as the API shows them; a run's info, events, step summaries, scope ancestors, checkpoints; tenant metrics; operation queues and requests |
  | Signals and input | list and get input requests, answer one, the pending-input page, a workflow's open actions and submitting one |
  | Compilation | compile now, publish as an agent, what an image was built from |
  | On request | run an agent's capability |

- **Parts of the API the engine serves itself** (`Engine::serve`). What is
  answered with a connection's credentials is the engine's: connections,
  the OAuth redirect, a tenant's object model, chat channels' webhooks,
  triggers, and a trigger's HTTP events. Each is a router the engine holds;
  the API authenticates the caller and hands the request over as it
  arrived. What the caller's role may do there is decided on the engine's
  side, where the route is known. The chat channel router, with its
  conversations, is then the engine's alone.
- **The boundary is held.** The server's state hands handlers the interface
  and neither the runtime client nor the execution engine, and a test reads
  the handlers' and MCP tools' sources and fails if one names them.
- **A run's records are read in its tenant's scope**, entered on the
  engine's side from the tenant the call names, instead of relying on the
  scope the handler happened to run in.

What is left of this phase:

- **Workflow handlers still hold the connections facade**, to check that a
  workflow's steps name connections that exist. It can do more than that
  (it is the way to seal), so the API side is not yet free of it. It needs
  a narrower question: "which connections does this tenant have".
- **Compiling through the queue is still the API's**: it writes the compile
  request to Valkey and reads its progress, and the engine's worker takes
  it from there. That is a queue already, so it crosses a process boundary
  as it is; it moves behind `Engine` when the API's Valkey user loses the
  queue (Phase 4).
- **The engine is not yet sorted out of the server's start-up.** Every
  worker is started in one function, and `ExecutionEngine` still holds the
  outbox and admission beside what `Engine` exposes. Phase 3 needs them
  apart to start one role without the other.
- **What crosses is not yet only values.** A forwarded request carries the
  caller the API authenticated as a request extension, and some operations
  take the caller's `AuthContext`; both are in-process shapes. Phase 3 puts
  them on the wire, and Phase 4 replaces them with the token itself.

Two small differences in answers, both for requests that were refused
anyway: an unknown path under `/api/runtime/object-model` or
`/api/runtime/triggers` is now refused as unauthenticated before it is not
found, and a wrong method on those routes is answered by the engine.

## Phase 3: two roles and the wire between them

1. **`RUNTARA_ROLE` is `all` (default), `api` or `engine`.** The same binary.
   - `api` opens no runtime pool, loads no encryption or wrapping key, no
     executor certificates and no component bundle, starts no engine
     worker, and refuses to start if it is given any of them.
   - `engine` binds no public listener.
2. **`Engine` over mutual TLS.** A remote implementation for the API role
   and a router for the engine role (`RUNTARA_ENGINE_LISTEN`,
   `RUNTARA_ENGINE_URL`), with the listener, certificate checks and
   3-second connect timeout the executor work already has in
   `runtara-executor`'s `tls` module. JSON bodies; event streams and
   run-and-wait as held responses with a deadline.
3. **Certificates name the role.** The API role's certificate is accepted on
   the engine listener only, an executor's on the run channel only; neither
   is accepted on the other.
4. **What the API role still does itself:** terminates requests, limits
   bodies and rates, verifies tokens and keys to refuse early, CRUD on
   definitions in the server database, graph validation (pure, already
   sandboxed when `RUNTARA_VALIDATOR_WASM` is set), the MCP protocol and its
   sessions, and sealing a newly entered secret to the tenant's public key.
5. **More than one API process** may run in front of one engine. The engine
   stays one process, as the server is today.

**Tests:** a new e2e script starts an API process, an engine process, a seal
process and two executors, and runs the scenarios the executor script runs,
plus: the API process's environment holds no key and no runtime database
URL; killing the API process loses no run; an engine that is away gives the
caller a clear error and no half-started run.

## Phase 4: the engine decides, not the API role

Without this phase the split protects keys and the runtime database but a
broken API role could still tell the engine "this is tenant X".

1. **The engine verifies the caller's authority itself.** The API role
   forwards the token or API key with each call; the engine runs the same
   verification (JWKS, the `api_keys` table, membership) and takes the
   tenant from that, never from the API role's word.
2. **Triggers are checked by the engine.** The API role forwards a webhook
   as it arrived; the engine looks up the trigger, has seal check the
   signature, and writes the outbox.
3. **Only the engine makes a workflow runnable.** Editing a draft is CRUD.
   Compiling, publishing and setting the current version are engine
   operations that need a user's authority, and the engine records what it
   approved in the runtime database. A definition changed in the server
   database behind the engine's back does not run.
4. **Database roles.** The API role's Postgres role has no rights on the
   outbox, admission and compilation tables, nor on anything in the runtime
   database, nor on `seal_wrapped_key` (empty once seal is separate). The
   role `runtara_system` is created and granted to the engine's role and
   not to the API role's, so the API role cannot ask for every tenant
   (Phase 0 built the check; nothing creates the role yet). Its lookups by
   id for unauthenticated requests, which today run as the platform, move
   to the engine with the triggers.
5. **Valkey users.** The API role's user cannot write the trigger,
   compilation or control streams.

**What a broken API role has after Phase 4:**

- the requests, responses and tokens that pass through it while it is
  broken, and so the ability to act as those users until their tokens
  expire;
- read and write on definition tables, for every tenant, because it chooses
  the tenant its queries run as. It can change a draft; it cannot make one
  run;
- denial of service.

It does not have connection secrets or the use of them, checkpoints or run
data beyond what those users may read, the ability to start a run as a
tenant, or a certificate an executor or the run channel accepts.

## Phase 5: deployment and documents

- An overlay for the evaluation stack that runs the three processes, next to
  `docker-compose.executor.yml`.
- `docs/` pages for the roles, their settings, and what each must not be
  given; `AGENTS.md` gains the rule that handler code reaches the engine
  through `Engine` only.
- A hosted deployment maps onto this directly: the proxy in front routes
  public traffic to API processes and refuses the engine, run channel,
  executor and seal ports; the engine and seal run on hosts the proxy does
  not share.

## What this does not protect against

- **Authorisation mistakes.** If the engine wrongly lets tenant A read
  tenant B's instance, a healthy API role passes that request along. The
  split limits what code execution in the public process is worth; it is
  not the defence for multi-tenancy. Phase 0's fail-closed row security is.
- **A broken engine.** It has both databases and may ask seal to use any
  credential it is a caller for. Phase 1 still holds: it cannot read one.
- **A broken seal host.** Everything.
- **Traffic in flight** through a broken API role, as above.

## Decisions

1. **In-process seal is the default.** A single process runs seal in itself,
   with the startup warning seal asks for. The split roles refuse it.
2. **Where the keys live once seal is separate: with seal.** There is no
   key service. Self-hosting keeps them in a directory on seal's host,
   with a wrapping key file. A hosted deployment keeps them in a bucket
   that its seal instances share, with the wrapping key in a KMS.
   In-process, they stay in the server database.
3. **The object model is served by the engine role** in Phase 3. A role of
   its own is considered later, if that traffic should not share a process
   with the engine.
4. **Agent metadata comes from the `.meta.json` files.** The API role lists
   agents and their capabilities from them, without loading components.
5. **Compilation is the engine's, and where it runs is the host's choice.**
   Requesting a compile is an engine operation. The engine compiles
   in-process unless the embedding host installed a compile executor
   (`compile_executor::set_compile_executor`), as a hosted deployment that
   compiles on separate instances does. That hook stays as it is and moves
   with the engine; this plan adds nothing to it. The API role never
   compiles.
6. **Re-wrapping keys is seal's.** Runtara's own re-wrapping is replaced by
   seal's `rewrap` (Phase 1, step 5).

## Order and size

Phases 0 and 1 are done, and Phase 2 but for its last items. Phase 2 was the
bulk: about 25 to 30 operations and 50 to 60 call sites, done in the six
groups above. Phase 3 is mostly transport once Phase 2 holds. Phase 4 changes
who verifies, and needs Phase 3. Each phase leaves a server that runs as one
process exactly as before.
