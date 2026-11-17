# Separating the public API from the engine

A plan. Phase 0 is implemented (branch `feature/row-security-fails-closed`);
the rest is not. This file is removed when the work lands and `docs/`
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

## Phase 1: seal in a process of its own

seal already runs three ways behind one client interface: in the
application, as a process on the same host over a Unix socket, or on another
machine over mutual TLS. This phase lets Runtara use the second and third.

As a process of its own, seal keeps its wrapped keys in a directory on its
own host (`<dir>/<keyset>/<kid>.json`) or in a bucket of an S3-compatible
object store, with the same names, that several seal instances share. The
wrapping key is a file on seal's host, or a key that stays in a KMS. seal
has no key service to read from any more. (Read at seal `02bd886`; the
client, protocol and sealed format are unchanged from the revision Runtara
pins.)

0. **Move to seal's current revision.** Nothing Runtara calls changed
   shape: `Keyring::new` now takes one wrapping key or several, and takes
   the one Runtara gives it as before. Build and run the `sealed` tests
   against it before anything else.
1. **Choose the mode by configuration.** `RUNTARA_SEAL_MODE` is
   `in_process` (default for a single process), `unix` or `mtls`, with a
   socket path or an address and certificates. `SealedCredentials` is built
   from `seal_client::unix` or `seal_client::mtls` instead of
   `seal_client::in_process`; it already reports its mode.
2. **Move what in-process mode did around the client.** Three things today
   reach into the keyring directly and must go through seal's calls instead:
   - creating a tenant's keyset (`GenerateKeyset`, an admin caller; seal
     stores the wrapped key in its key source),
   - reading a tenant's public key to seal a new secret (`PublicKeys`,
     cached; sealing needs no private key),
   - destroying a tenant's keys (`DestroyKeyset`, a caller with
     `destroy_keysets`; seal deletes them from its key source, and in a
     bucket every version of them).
3. **Move the keys.** In-process, the keys stay where they are: wrapped, in
   `seal_wrapped_key` in the server database. For a separate seal, a command
   (`runtara-server seal-keys export --out <dir>`) writes them from that
   table in the layout seal's directory reads, with seal's own
   `DirKeySource` so the layout is seal's and not a copy of it. A bucket
   holds the same files, so a deployment that uses one copies the
   directory into it. The keys and the wrapping key go to seal; the
   wrapping key leaves the server's environment, and with a separate seal
   the server refuses to start if it is still given one. Once seal answers for every
   keyset the table held, its rows are deleted: a copy left in the database
   is a copy the wrapping key still opens.
4. **Say what running seal now asks of the operator.** seal is the only
   place the keys are, so:
   - **one seal, self-hosted:** a key directory and a wrapping key file on
     its host. The directory is backed up; a lost directory is every sealed
     secret lost.
   - **several seals, or a hosted deployment:** the keys in a bucket with
     versioning on, and the wrapping key in a KMS, so that no machine holds
     a wrapping key file and a copy of the bucket opens nothing. The store
     must support conditional writes.
   - **replacing the wrapping key** is seal's own procedure, while seal
     runs: the new key becomes `keys.wrap`, the old one stays in
     `keys.unwrap_also`, and `seal rewrap` wraps every key again in place.
     The same steps move from a file to a KMS.
5. **Use seal's re-wrapping in-process too.** seal's `rewrap` now works on
   any key source that can list and replace its keys. Runtara's database
   key source gains those two operations, and Runtara's own re-wrapping
   after an encryption key rotation (`follow_encryption_key_rotation`,
   `rewrap_retired`) is replaced by seal's, with the retired keys given as
   the older wrapping keys.
6. **Audit and limits follow seal.** In-process, Runtara writes seal's audit
   records to its log and sets the buffer caps. A separate seal writes its
   own; Runtara reads the caps from `Info`.
7. **Ship it.** A `seal` service in `docker-compose.executor.yml` (or a
   sibling overlay), its certificates issued by the existing PKI step, and a
   page in `docs/`.

**After this phase** a broken server can ask seal to *use* a credential,
toward the destination sealed into it, and every use is in seal's audit log.
It cannot read one. It still has both databases.

**Tests:** seal's client behaves the same in all three modes by its own
suite; Runtara's `sealed` tests run against a seal process as well as
in-process, and one e2e script saves a connection, runs a workflow that uses
it, and shows the server process holds no wrapping key.

## Phase 2: one interface between handlers and the engine

No behaviour changes and no new process. This is most of the work, and it
is reviewable one operation group at a time.

1. **A trait, `Engine`, in a small crate of its own** (`runtara-engine-api`:
   the trait, its request and reply types, and later the client and the
   router). Its first implementation wraps today's `RuntimeClient` and the
   engine half of `ExecutionEngine`.
2. **Every call carries who is asking.** Each method takes the caller's
   authority (the verified identity today; in Phase 4 the token or key
   itself) and the tenant, instead of reading a task-local. The tenant
   scope of Phase 0 is a task-local and does not cross a process boundary:
   the engine enters the scope again from what the call carries.
3. **Move the operations behind it, a group per change:**

   | Group | Operations |
   |---|---|
   | Launch | start a run, run and wait (`run_sync`), accept a trigger event, replay |
   | Lifecycle | stop, pause, resume |
   | Signals and input | submit an action or signal, list and get input requests, session delivery |
   | Reads | executions (get, list, summary, listing options), instance info, events, step summaries, scope ancestors, checkpoints, tenant metrics, operation queues and requests |
   | Compilation | compile, compile as an agent, is a compile pending, wait for one, progress |
   | Credentialed | resolve a connection's resources, exchange an OAuth code, revoke a token, register and verify a webhook secret, send a channel reply, test an agent capability, open an object model session |

4. **Split `ExecutionEngine` along the line.** Admission, the outbox, the
   relay, the trigger workers and the in-memory admission maps are the
   engine's. What is left on the handler side is a call to `Engine`.
5. **Handlers, channels and MCP hold `Arc<dyn Engine>`** and nothing else of
   the engine: not `RuntimeClient`, not `ExecutionEngine`, not the sealed
   credentials, not the component dispatcher. A test fails the build if
   `api/handlers`, `channels` or `mcp` import those types.
6. **Workers are sorted by role.** Engine: trigger workers, compilation
   worker, outbox relay, admission reconcilers, control children, session
   delivery, cron, channel intake worker, OAuth sweep, artifact restaging,
   and everything `runtara-environment` starts. API: the control-event
   consumer that revokes a removed member's keys, product events, pool
   monitors.

**Tests:** the existing server suite passes unchanged at every step, which
is the point of doing this phase with no behaviour change.

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

Phase 0 is done. Phase 1 is independent of the rest. Phase 2 is the
bulk: about 25 to 30 operations and 50 to 60 call sites, done in the six
groups above. Phase 3 is mostly transport once Phase 2 holds. Phase 4 changes
who verifies, and needs Phase 3. Each phase leaves a server that runs as one
process exactly as before.
