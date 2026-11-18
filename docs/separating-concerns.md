# Separating the public API from the engine

A plan. Phases 0 (branch `feature/row-security-fails-closed`), 1 (branch
`feature/seal-out-of-process`, on top of it), 2 (branch
`feature/engine-interface`, on top of that) and 3 (branch
`feature/engine-roles`, on top of that) are implemented; the rest is not. This file is removed when the work lands and `docs/`
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
| Tenants' public keys (to seal new secrets) | no | yes | — |
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

## Phase 2: done

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
  | Compilation | compile now, publish as an agent, what an image was built from; queue a compile, wait for it, and read its progress |
  | Connections, as a workflow names them | which connections a tenant has (ids, kinds, titles); take down a removed trigger's webhook |
  | On request | run an agent's capability |

- **Parts of the API the engine serves itself** (`Engine::serve`). What is
  answered with a connection's credentials, or by what the engine loaded,
  is the engine's: connections, the OAuth redirect, a tenant's object
  model, chat channels' webhooks, triggers, a trigger's HTTP events, and
  agents. Each is a router the engine holds; the API authenticates the
  caller and hands the request over as it arrived. The engine answers it in
  the scope of the tenant it is for, which it enters itself, and decides
  there what the caller's role may do, where the route is known. The chat
  channel router, with its conversations, is the engine's alone.
- **The boundary is held.** The server's state hands handlers the interface
  and neither the runtime client nor the execution engine. A test reads the
  sources of the API's handlers and MCP tools and fails if one names the
  engine's own types, the connections facade, seal, the webhook manager,
  the component dispatcher or the compile queue. The handlers of the parts
  the engine serves live beside the API's and are named in that test as the
  engine's.
- **A run's records are read in its tenant's scope**, entered on the
  engine's side from the tenant the call names, instead of relying on the
  scope the handler happened to run in.
- **Background work is started for the role it belongs to**
  (`crates/runtara-server/src/roles.rs`). Every worker is started through
  one helper that names its role, and the server logs what it started for
  each. One process runs both roles, so what runs is unchanged.

  | Role | Work |
  |---|---|
  | Engine | trigger workers, compilation worker, execution outbox relay, admission reconcilers, control children, conversations, cron, unconfirmed OAuth sweep, platform OAuth apps reload, artifact restaging, invocation and trigger stream clean-up, object model pool monitor, and everything `runtara-environment` starts |
  | API | member control events (revoking a removed member's API keys) |
  | Each process, whatever its role (since Phase 3) | the product events drain, the database pool monitor, the heartbeat log |

What Phase 3 takes from here, none of it left undone by this phase:

- **What a call carries.** Each operation names its tenant, and the ones a
  signed-in caller answers for carry that caller as the API verified it; a
  forwarded request carries the same as a request extension. That is the
  verified identity this phase set out to pass. Phase 3 writes it on the
  wire, and Phase 4 replaces it with the token itself, which the engine
  verifies.
- **Roles are declared and not yet chosen.** `Roles::all()` is the one
  value used. Phase 3 reads it from `RUNTARA_ROLE`, and has the set-up
  around each worker (the clients it builds before it starts) follow the
  role as the start already does.
- **Agents are served whole by the engine.** The decision to list them on
  the API's side from the `.meta.json` files without loading components is
  still open to Phase 3; until then the API's side simply does not hold
  them.

Small differences in answers, all for requests that were refused anyway: an
unknown path under `/api/runtime/object-model`, `/api/runtime/triggers`,
`/api/runtime/agents` or `/api/runtime/_internal` is now refused as
unauthenticated before it is not found, and a wrong method on those routes
is answered by the engine.

## Phase 3: done

The server runs as its API alone, its engine alone, or both, and the two
talk over mutual TLS. Implemented on `feature/engine-roles`; `docs/roles.md`
says how to run it.

- **`RUNTARA_ROLE` is `all` (the default), `api` or `engine`**, in the same
  binary (`crates/runtara-server/src/roles.rs`). Start-up is in three
  parts: what every role establishes first, the engine's path (which also
  builds the API's routes when it runs them), and the API's own.
  - **`api`** opens no runtime database, loads no encryption or wrapping
    key, reaches no seal, loads no component and starts none of the
    engine's work. It refuses to start if it is given the way to any of
    them: `RUNTARA_DATABASE_URL`, the connection encryption keys, the seal
    wrapping key or seal's address, an executor certificate directory, the
    artifact store, or an address to listen on as an engine. Its routes
    are built by a function that is handed the database, Valkey, the
    engine interface, the agent catalog, product events and what
    authenticates a caller, and nothing else.
  - **`engine`** binds no public listener, and starts only with a seal of
    its own (decision 1).
  - **`all`** is unchanged, and also listens for the API role when it is
    given `RUNTARA_ENGINE_LISTEN`.
- **`Engine` over mutual TLS** (`crates/runtara-server/src/engine/`). Each
  operation is written once, as a named message with its answer
  (`wire.rs`); the engine's router answers one in the scope of the tenant
  it names (`listener.rs`); the API's side is an `Engine` that asks
  (`remote.rs`). It uses the listener, certificate checks and 3-second
  connect timeout of `runtara-executor`'s `tls` module. Bodies are JSON. A
  call that is held until work is done (run and wait, compile, run an
  agent, a request the engine serves) has a deadline of fifteen minutes,
  the others one minute; a request the engine serves is handed over as it
  arrived and its answer streamed back.
- **Certificates name the role.** `runtara-executor init-pki --api-dir`
  issues a third certificate, for `runtara-api`, good for calling only.
  The engine's listener accepts that one alone, and the run channel an
  executor's alone.
- **What the caller is.** A request handed over carries the caller the API
  authenticated in a header of the API's own, which is dropped if it came
  from outside. This is the API's word, as it was in one process; Phase 4
  replaces it with the caller's token.
- **An engine that is away** is the error of the operation that was asked,
  saying so (a 503 for starting a run, and for any request the engine
  serves). A call that was never sent says nothing was done; one that was
  sent and not answered says it may have been. The API starts, and keeps
  answering for its own records, whether or not the engine answers.
- **More than one API process** runs in front of one engine. The engine
  stays one process.
- **Background work by role.** The engine starts all of its own. The API
  starts the consumer of member control events. The watch on the database
  pool, the product events drain and the heartbeat are each process's own,
  whatever its role.

Three things the API's code read from the process it ran in turned out to
be the engine's, and are now asked of it:

- **Whether a compiled workflow is current** was judged by this process's
  lowering mode, component release and installed trusted built-ins. The
  engine says what it compiles by and runs, and the API judges by that.
- **A tenant's published workflow-agents** were read from a staging
  directory beside the artifact store. The API asks the engine for them,
  and asks it to take one out of use when its workflow is deleted.
- **What agents there are** (decision 4): the API reads the
  `runtara_agent_*.meta.json` files of the release the engine runs, from
  `RUNTARA_AGENT_COMPONENTS_DIR`, and loads no component. It uses them to
  check what a workflow names. The routes that list agents and run one on
  request stay the engine's.

Left as it was, on purpose:

- **A newly entered secret is sealed by the engine**, not by the API. The
  plan had the API seal it to the tenant's public key. Connections are a
  part of the API the engine serves whole (Phase 2), so the secret crosses
  the API in the request that carries it either way, and from there goes
  to the engine over mutual TLS.
- **Database migrations run in whichever process starts.** An API given a
  role that cannot change the schema (Phase 4) starts with
  `SKIP_MIGRATIONS=true`, after the engine.
- **The engine had no health endpoint of its own.** What said it was up
  was the API's log (`The engine answers`) and its answers. Phase 5 gave it
  one (`RUNTARA_ENGINE_HEALTH_LISTEN`).

**Tests.** The wire has unit tests in one process and over real mutual TLS
(`engine::remote::tests`): a request crosses as it arrived, a caller named
from outside is not the caller, an operation is answered as the engine in
this process answers it, each channel takes its own certificate only, and
an engine that is not listening is never sent anything. Two scripts run it
end to end:

- `e2e/test_two_roles.sh` starts seal, an engine, two executors and an API
  whose environment is written out in full: what each role refuses and
  holds; runs through the API on both executors; a run that is waited for;
  a connection sealed by seal and used; which certificate each channel
  takes; runs that outlive a killed API; an engine that is away, to which
  the API answers 503 and starts nothing, and that comes back; a second
  API.
- `TWO_ROLES=true e2e/test_served_tenants.sh` runs the two-tenant test
  with the server as two processes on database roles bound by row-level
  security, which is what shows the tenant's scope entered on the engine's
  side.

## Phase 4: done

The engine decides, not the API role. Implemented on
`feature/engine-decides`; `docs/roles.md` says how to set it up.

1. **The engine verifies the caller's authority itself.** The API holds
   what a caller presented (the token or API key, the tenant the path
   named, which way in) for the length of the request
   (`crates/runtara-server/src/auth/presented.rs`), and sends it with every
   call it makes for that caller. The engine's listener runs the API's own
   verification on it (`middleware::auth::verify`: JWKS, the `api_keys`
   table, the tenant's status, membership) and goes by the caller it finds
   (`engine/listener.rs`):
   - an operation for a tenant is answered only for a caller who acts for
     that tenant, in that tenant's scope;
   - an operation, whether it changes something or reads, is held to the
     caller's role and API key scope: it names the permissions of the API's
     routes that ask it (`engine/wire.rs`, `needs`), and a test holds the
     two to each other. Only the list of a tenant's published agents needs
     none, as the API's agent routes do;
   - a part of the API that has a caller is served to the caller the engine
     found; the caller the API names is not read.

   MCP tool calls and streamed answers keep what was presented, as they
   keep the tenant's scope.
2. **Triggers are checked by the engine.** They have been since Phases 2
   and 3: webhooks, channel webhooks and the synchronous trigger are parts
   of the API the engine serves, handed over as they arrived. The engine
   looks up the trigger, has seal check the signature and writes the
   outbox. What Phase 4 adds is that the API's database role cannot reach
   those tables, and cannot look up a row across tenants.
3. **Only the engine makes a workflow runnable** (`engine/approvals.rs`).
   The engine records, in the runtime database, each version someone who
   may run it asked to have compiled or run, as the definition was then,
   the image the engine compiled from it, and the version chosen to run
   when none is named. Every launch goes by those records: a definition
   changed in the server database since is refused; an image the engine did
   not compile from it is compiled again and not run. Setting the current
   version and forgetting a compilation are engine operations, and the API
   no longer writes `workflow_compilations`. What was compiled and chosen
   before is carried over once, at the first start.
4. **Database roles** (`scripts/roles/api-database-role.sql`). The API's
   role is granted its own records, the compilation tables to read, and
   nothing of the outbox, admission, connections, keys or the runtime
   database. `runtara_system` is created and granted to the engine's role
   alone, so the API's role cannot ask for every tenant. The one lookup the
   API made across tenants, whose an API key is, is asked of the engine.
5. **Valkey users** (`scripts/roles/api-valkey-user.acl`). The API's user
   cannot write the trigger, compilation or control streams, an admission
   count or a membership.

Where it differs from the plan as it was written:

- **The session delivery routes were gated.** Tracing which routes ask each
  operation found three with no permission. They are gone since, with the
  sessions: a chat with a workflow is a conversation
  ([native-chat.md](native-chat.md)).
- **The API's role keeps `invocation_trigger` to update**, because deleting
  a workflow switches its triggers off in the same transaction.
- **A compile nobody approved may still happen** (through the compile
  queue, were the Valkey rules not applied). Its image is not approved and
  does not run.
- **The engine verifies on every call**, with no cache: a request that asks
  the engine five things is verified five times.

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

**Tests.** Unit tests cover what crosses (`auth::presented`), the engine's
side of the wire (`engine::remote::tests`: no caller, another tenant's
caller, a credential that allows less, a caller the API names, and each
operation's permissions against the route map) and the
verdict on a version (`engine::approvals`). `TWO_ROLES=true
e2e/test_served_tenants.sh` runs the two-tenant test with the API on its
own database role and Valkey user, and its last section turns each thing
the API holds against the engine.

## Phase 5: done

Deployment and documents. Implemented on `feature/engine-decides`.

- **`docker-compose.roles.yml`** is an overlay for the evaluation stack that
  runs the three processes, next to `docker-compose.executor.yml` and on top
  of `docker-compose.seal.yml`. The `runtara` service becomes the engine and
  a new `api` service takes the requests. It makes the certificates, the
  API's database role and the two Valkey users from the files under
  `scripts/roles/`, which a test holds it to.
- **The engine says it is up** where `RUNTARA_ENGINE_HEALTH_LISTEN` tells it
  to: `/health` and nothing else, once it is ready. The overlay's health
  check asks it, and the API starts after it.
- **`docs/roles.md`** is the page for the deployment: what each process is
  given, the certificates, the database roles and Valkey users, the
  overlay, and how a hosted deployment maps onto it. The proxy in front
  routes public traffic to API processes and refuses the engine, run
  channel, executor and seal ports; the engine and seal run on hosts the
  proxy does not share.
- **`e2e/test_two_roles_oidc.sh`** closes what Phase 4's scripts left to
  unit tests: the engine verifying OIDC tokens, membership and roles
  itself.

## After these phases

The engine is still one process that holds everything. Taking the compiler
and the receiving of anonymous traffic out of it is planned in
[splitting-the-engine.md](splitting-the-engine.md).

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
   with the startup warning seal asks for. The split roles refuse it: the
   engine role starts only with a seal of its own, and the API role is
   given no way to one.
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

Phases 0 to 5 are done. Phase 2 was the
bulk: about 25 to 30 operations and 50 to 60 call sites, done in the six
groups above. Phase 3 was mostly transport once Phase 2 held, and found
three more things the API's code took from its own process. Phase 4 changes
who verifies, and needs Phase 3. Each phase leaves a server that runs as one
process exactly as before.
