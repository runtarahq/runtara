# Splitting the engine: the compiler and intake

A plan, with Phase 6 built and Phase 7 built for a trigger's webhooks. What
is left of Phase 7 is said where it is. It follows
[separating-concerns.md](separating-concerns.md), whose phases 0 to 5 put
the API in a process that holds nothing of the engine's and made the engine
decide for itself who a call is for and what runs.

## Why

After those phases one process is still worth everything: the engine. It
has both databases, may ask seal to use any credential, and takes three
kinds of input it did not write:

- **workflow graphs**, which it compiles;
- **anonymous requests from the internet**, handed over as they arrived:
  webhooks, chat channel callbacks, OAuth redirects;
- **requests from callers** for the parts of the API it serves: connections,
  triggers, the object model, agents.

A flaw in how any of those is parsed is code running where every tenant's
run state is. This plan takes the first two out of that process. It was
traced against the code before it was written; where the trace changed what
seemed obvious, it says so.

It does **not** split the engine by database. Both halves of anything one
could cut need both databases; what can be cut is what handles input from
what holds state.

## Phase 6: the compiler holds nothing

### What the trace found

- **Compiling touches no secret.** No seal, no connection, no credential;
  the compile service is not given the connections facade.
- **The pure compile step is already separable.** `compile_executor` is a
  hook: an executor is handed the parsed graph, its children, the agent
  catalog and the component directories, and returns a build. Everything
  after it stays with the engine: the trusted-pin and memory-floor checks,
  the `workflow_compilations` row, registering the image (runtime `images`
  and the artifact store), the approval record, the dependencies.
- **The compiler already exists as a guest.** `crates/runtara-compile-guest`
  is the compiler as a WASI command. It takes one request over the workflow
  API (`GET /v1/run/input`), fetches each component by its SHA-256 as it
  first reads it (`GET /v1/artifacts/{sha256}`), puts back the composed
  `workflow.wasm` and posts its outcome. `runtara_workflows::compile_request`
  makes the request and applies the result.
- **So does the host's side of serving one.** The workflow API serves a
  run its input, artifacts by digest (to fetch and to put back) and its
  outcome. `runtara-component-host` queues compiles for a guest and serves
  a tenant's warm compiler (`compile_worker`,
  `ApiLaunches::begin_compile_worker`). `scripts/check-compiler-guest.sh`
  checks the guest against the native compiler. What the repository does
  not have is an executor of compiles that uses them: nothing calls
  `set_compile_executor`. A hosted deployment installs its own.
- **A compiler with a database would need most of the engine's.** The whole
  compile path reads `workflow_definitions` and `workflows`, writes
  `workflow_compilations`, `workflow_dependencies`, runtime `images` and
  `workflow_approvals`, the artifact store, and three globals the engine
  sets from its runner and dispatcher (the trusted pins, the stdlib mode the
  lowering tag depends on, the component release). Moving all of that would
  give the process that parses graphs the means to say what runs.

So the split is not a "compiler role" with a database role of its own. It
is the compile step as a guest that is given nothing.

### What was built

The first thing to find out was whether the engine could host the guest as
it is. It cannot, cheaply: the guest reaches its host over `wasi:http` 0.2,
and the engine's interception of the workflow API is for runs on
`wasi:http` 0.3. But the repository already runs one tenant-facing step in
a sandbox of its own: save-time validation (`validation_sandbox.rs`, a
Wasmtime store with no filesystem, network or environment, and bounds on
fuel, memory and time). The compiler was built the same way.

1. **The compiler as a WASI command for that sandbox**
   (`runtara-compile-sandboxed`, in `crates/runtara-compile-guest`). It
   takes one request on its input, reads the component files the request
   names where its host shows them, writes its outcome and the composed
   component, and ends. It is the compile the guest for a host elsewhere
   runs (`compile_request::compile`), with files in place of fetches.
2. **The sandbox** (`crates/runtara-server/src/compile_sandbox.rs`), with
   `RUNTARA_COMPILER_WASM` naming the command. A Wasmtime store of its own
   for each compile: no network, no environment, a bound on its memory
   (3 GiB) and its time (five minutes), and of the filesystem only the
   components directory and the tenant's published agents, read-only. It
   is installed through the existing hook, so everything after the build
   stays as it was, and a host that set an executor of its own keeps it.
3. **An engine in a process of its own refuses to compile anywhere else**
   (decision 1): it does not start without the compiler, or with one that
   does not load. A single process keeps the choice.
4. **The engine does not take the build on its word.** The component is
   checked against the digest the outcome names before anything is written,
   and again at registration. The agents a build says it composed, which a
   launch checks against the tenant's allowlist, must include every agent
   the engine finds in the graph and its children, or the build is refused.
   The trusted pins are compared with what the engine has installed, and
   the memory floor with the version's setting, as before.
5. **One compile cannot reach the next**: each has a store of its own,
   which is dropped when it ends.
6. **It ships with the components**: `scripts/build-agent-components.sh`
   builds it and the bundle carries it. `docker-compose.roles.yml` gives it
   to the engine.

What stays in the engine: the compile queue and its worker, loading the
source, every check and record after the build, the approval records.

**Where it differs from the plan as it was written.** A compile is not
placed as a run on an executor: the sandbox is in the engine's process,
which is the same confinement the engine gives a workflow it runs itself
(a Wasmtime store), and not a process of its own. So a flaw in Wasmtime
itself is not contained, as it would be by an executor's sandbox. Placing
compiles on executors is built since
([compiler-on-an-executor.md](compiler-on-an-executor.md)), for an engine
whose runner is executors: the obstacle
named here at first (the guest and the engine's embedded workflow API
speaking different `wasi:http`) does not hold on an executor, whose runs
are given both. The stages a compile reports
from inside (for the progress shown while it runs) are not reported from
the sandbox; the stages around it are.

### What a broken compile is worth

A graph that breaks the compiler runs code in a guest with no credential and
no address. It can return a wrong program for the version it was asked to
compile: a version of that tenant's workflow, which runs as a sandboxed
guest with what that tenant's workflows are given. It cannot read another
tenant's graph, change another version, or reach a database.

### Tests

- Unit: what a compile wrote is parsed and refused when it is not an
  outcome; a component that is not the one named is refused; a build that
  leaves out an agent the graph calls is found out.
- With the compiler (`compiler-sandbox-tests`, as `validator-sandbox-tests`
  is for the validator): the sandbox makes the bytes a compile in this
  process does; a graph that does not compile fails with the same error; a
  components directory that is not there is not shown.
- E2E: `test_two_roles.sh`, `TWO_ROLES=true test_served_tenants.sh` and
  `test_two_roles_oidc.sh` run with every compile in the sandbox, on
  executors and on the embedded runner. `test_two_roles.sh` checks that an
  engine with no compiler, or one that does not load, refuses to start.

## Phase 7: intake

### What the trace found

The engine serves eight parts of the API. They are not alike.

| Part | Needs the runner? | seal | Server database |
|---|---|---|---|
| Events: `events/http/{trigger}/{action}` | No. It inserts into the outbox and answers `queued`. | `verify`, for a Mailgun trigger only | reads `invocation_trigger`, a connection; writes the outbox tables |
| Events: `events/http-sync/{workflow}` | **Yes, synchronously**: it runs and waits. | none | as a run |
| EventsForCallers | The same two routes with a caller, for MCP. | as above | as above |
| Channels: chat webhooks | **Yes, in process**: after it answers, a session loop of up to ten minutes polls the run, reads its events and delivers replies to it as input. | `verify` to check the webhook; `send` for replies | `channel_intake`, triggers, a connection, then as a run |
| Triggers | No | seals a webhook secret; `send` to register it | `invocation_trigger`, a connection |
| Connections | No | seals on save; `send` to revoke and discover | connection tables, rate limit events |
| OAuthCallbacks | No | seals tokens; `send` for the code exchange | a connection, `oauth_state` |
| ObjectModel | No | `connect`, to the tenant's own database | a connection |
| Agents: listing | No. It reads descriptions. | none | none |
| Agents: `/test` | **Yes**: it runs a component in process, with host services that reach the runtime. | `send`, `connect` | a connection |

Four more things bear on the design:

- **Starting a run from a webhook needs no runtime database.** The outbox
  write is one transaction on the server database, bounded by an admission
  counter (in Valkey, or a table). The count of running instances it also
  consults is a cache refreshed in the background, and without a runtime it
  counts none; the counter is what holds. A webhook's event carries no
  version, so what runs is still resolved and checked against the approval
  records by the runner at launch.
- **Chat channels already have a durable hand-off.** A webhook is answered
  once its message is stored in `channel_intake`; a worker claims what is
  due. The cut between receiving and running is that table.
- **Reading a connection needs the at-rest key.** A connection's parameters
  are encrypted at rest as a whole, with
  `RUNTARA_CONNECTIONS_ENCRYPTION_KEY`, and the sealed secrets are inside
  that. So even checking a webhook's signature, which only asks seal to
  `verify`, first decrypts the row with a key the API role is refused.
- **Sealing a new secret is an admin call.** It is done locally against the
  tenant's public key, but fetching that key (`public_keys`) is allowed to
  an `admin` caller only, which may also generate and rotate keysets. Every
  part that takes in a secret needs it: saving a connection, creating a
  channel trigger, finishing an OAuth flow.

So "intake" is two different things. Receiving anonymous webhooks needs
almost nothing: read a trigger, ask seal to verify, write a row. Serving
connections and the object model needs nearly everything seal offers.
Moving both together would make a second process almost as valuable as the
engine; moving the first alone makes a small one.

### What was built, and what was not

Built, and described in [roles.md](roles.md):

- **The role** (`RUNTARA_ROLE=intake`), for a trigger's HTTP events and the
  synchronous trigger. It refuses the runtime database, the at-rest key, an
  executor's certificate and the engine's own settings, and starts only
  with a seal of its own. The API hands it those routes where
  `RUNTARA_INTAKE_URL` names it, and the engine otherwise.
- **Preparation 1**: a caller's synchronous run (MCP) is an operation the
  API asks as that caller, and the part that served it is gone. What anyone
  may send is answered by intake asking the engine one operation, to run a
  workflow and wait, which only intake's certificate may ask.
- **Preparation 3** (decision 2): what checks a trigger's webhook is kept
  on the trigger's row (`invocation_trigger.webhook_verification`): the
  connection, its kind, and its signing secret as stored, sealed. The
  engine writes it when a trigger is saved and every thirty seconds. Intake
  checks from that with seal; a trigger bound to a connection with nothing
  kept for it is refused.
- **Preparation 5**: where one tenant is served, a webhook that names no
  tenant is looked for in that tenant, not across tenants.
- **Preparation 6**: the per-tenant OAuth callback's scope, as a fix of its
  own.
- **A certificate** (`init-pki --intake-dir`), **a database role** and **a
  Valkey user** (`scripts/roles/intake-*`), and a seal caller that
  administers nothing.

Not built:

- **Preparation 2: chat channels.** Not moved to intake: removed. The
  platform-specific channels and the Channel trigger are gone from the
  core ([native-chat.md](native-chat.md)), so the engine receives no
  anonymous traffic for them. A chat platform is to reach a workflow as an
  integration on the native conversation, whose webhook is an ordinary
  trigger's, which intake receives.
- **Preparation 4: agents.** Done since: what agents there are and what
  each can do are operations of the engine (`ListAgents`, `AgentNamed`,
  `ComponentsStatusOf`), as running one on request already was, and the
  agent routes are the API's own. Which agents a tenant's plan includes is
  decided by the API, which reads the plan; the engine says what it loaded.
- **OAuth redirects** stay with the engine, as the plan had them: finishing
  a flow seals tokens.
- **Routing at the proxy.** The API hands intake what it receives; a
  deployment that wants anonymous traffic never to reach the API either
  routes those two paths to a listener of intake's own, which does not
  exist: intake listens for the API only.
- **A seal permission to verify alone.** seal's callers are held to
  keysets and to administering or not. Intake's caller administers nothing
  and its keyset rule gives it `send` and `connect` as well as `verify`;
  what stops it using a connection is that it cannot read one: its
  database role has no connection table and it has no at-rest key. A
  permission for `verify` alone is seal's to add.

### The design

A third role, **`intake`** (`RUNTARA_ROLE=intake`): what receives what
anyone may send. It is the only one of the engine's processes that the
internet's anonymous traffic reaches.

**What it serves**

- `events/http/{trigger}/{action}`: looks up the trigger, has seal check
  the signature where there is one, writes the outbox, answers `queued`.
- The chat webhooks, as far as the hand-off: checks the webhook with seal,
  stores the message in `channel_intake`, answers.

**What it holds**

| | Intake |
|---|---|
| Server database | A role of its own: reads `invocation_trigger` and connections' webhook verification material; reads and writes `execution_requests`; inserts into `execution_outbox`, `execution_admission_reservations`, `channel_intake`; the admission table when the counter is not in Valkey. Not a member of `runtara_system`. |
| Runtime database | **None.** |
| seal | A caller of its own that may `verify`, for every tenant's keyset, and nothing else: no `send`, no `connect`, no admin, no destroying. |
| Valkey | A user that may count admissions (`*:admission:reserved`) and read a tenant's status. Not the trigger stream: the relay that publishes the outbox stays with the runner. |
| Certificate | One of its own (`runtara-intake`), which the runner's listener accepts for the operations below only. |
| The at-rest key | None, after step 3 below. |

**What stays with the runner** (the engine, less intake): the outbox relay
and the trigger worker, the chat session loop and its worker (it claims
from `channel_intake`, runs, and sends replies through seal), the cron
scheduler, and for now every part served for callers.

**Before intake can be cut out**, in this order. Each is a change inside
the engine that leaves one process running as before:

1. **The synchronous trigger becomes an operation.** `events/http-sync` is
   answered by an operation of the runner (run a workflow and wait, for
   anyone), which intake asks. Intake vouches for nothing by asking: the
   route is open to anyone by decision (Phase 0) and runs any workflow by
   its id, so the runner does for intake what it does for the sender. With a caller (MCP's
   `execute_workflow_sync`), the API asks the existing `RunSync` operation,
   and the `EventsForCallers` part goes away.
2. **Chat channels are cut at `channel_intake`.** Receiving (verify, store,
   answer) is separated from the session loop, which today holds the
   runtime client and the execution engine directly and not through
   `Engine`. The loop becomes the runner's worker over the table.
3. **A webhook is checked without the at-rest key.** What seal needs to
   verify a webhook (already sealed, so safe to store as it is) is kept
   where it can be read without decrypting the connection: with the
   trigger (decision 2). This is the one schema change.
4. **Agents stop being a served part.** Listing moves to the API, which
   already reads the descriptions and asks the engine for a tenant's
   published agents. Running one on request is already an operation
   (`test_agent`); its route calls it.
5. **Tenant-less webhook paths.** Intake is not to look across tenants. A
   path that names its tenant enters that scope; where one tenant is
   served, a tenant-less path is that tenant's; the chat connection lookup
   that has no tenant filter gets one.
6. **The per-tenant OAuth callback's scope** (found by the trace, not yet
   confirmed by a run): `/api/oauth/{tenant}/callback` scopes itself on the
   API's side from the path, but records no requested tenant, so the engine
   that serves it may run it in no scope, and row-level security would then
   refuse it. To be confirmed and fixed whatever else is done: it is in
   what `main` has now.

**Then the cut**, as Phase 3 was for the API: the role, its refusals (the
runtime database, the at-rest key, a seal that may do more than verify), a
listener, its certificate, and the API (or the proxy in front) routing the
two webhook paths to it. Then, as Phase 4: its database role and Valkey
user under `scripts/roles/`, and its seal caller.

### What a broken intake is worth

- It can queue runs: of versions the engine approved, for any tenant it
  names, with any input. This is the cost of the design. It is bounded by
  the admission counter, and it is what the sender of a webhook can already
  do for a trigger whose id they hold.
- It can read triggers and the stored webhook verification material, which
  is sealed, and ask seal whether a signature is good.
- It can store chat messages for the runner to process.

It cannot read a run, a checkpoint or an event; use a connection (seal
gives it `verify` only); read a connection's parameters; reach a tenant's
object model; change what runs; or look across tenants in one query.

### The parts served for callers: not in this plan

Connections, OAuth callbacks, triggers and the object model stay with the
runner. Moving them would isolate the runner from more request parsing, but
the process that took them would hold a seal caller that may `send`,
`connect` and administer keysets, and so the use of every credential and
the contents of every tenant's object model. That is a smaller gain for a
larger change, and it wants two things first: a seal permission to read
public keys without administering keysets, and a decision on whether the
object model is a role of its own (decision 3 of separating-concerns). To
be planned with the hosted deployment.

### Tests

- Unit: the wire's tests extended to intake's certificate (it may ask the
  two operations it is given and no other); the admission bound without a
  runtime; a webhook verified from the new column.
- E2E: a script with seal, a runner, intake and an API. Webhooks and a chat
  message arrive through intake and run; intake's environment holds no
  runtime database and no at-rest key; its seal caller is refused `send`
  and `connect`; its database role is refused the runtime database and
  everything but its tables; with the runner away a webhook is still
  accepted and runs when it is back; a webhook for an unapproved version is
  queued and refused at launch. `TWO_ROLES=true test_served_tenants.sh`
  grows a third process.

### Size

Larger than Phase 6. The six preparations are most of it, and the chat
channel cut (step 2) is the largest: about the size of Phase 2's larger
groups. The cut itself is Phase 3 again with one more role, and smaller,
since the wire, the certificates and the verification exist.

## Order

1. **Phase 6**, alone: done.
2. **Phase 7's preparations 1, 2, 4 and 6.** Each is worth having in one
   process: they put the last direct uses of the runtime behind `Engine`
   and fix a likely fault.
3. **Preparations 3 and 5, then the cut**, when a deployment takes webhook
   traffic it does not want near run state.

## Decisions

1. **An engine in a process of its own refuses to compile in process**,
   once the guest has run in the suites, as it refuses seal in itself. A
   single process keeps the choice.
2. **A webhook's verification material lives with the trigger**, which is
   what intake already reads, so that checking a webhook needs no at-rest
   key (preparation 3).
3. **Intake answers the synchronous trigger by asking the runner**, so that
   one process takes all anonymous traffic. Workflows are trigger-agnostic:
   the route runs any workflow of a served tenant by its id, with no
   trigger record and no switch on the workflow. The runner's operation is
   therefore the route as it stands, and is not narrowed to some workflows.

## Left open

- **Whether the at-rest key is still needed.** Secrets are sealed inside
  the parameters it encrypts, so what it still hides is what is not secret
  (addresses, account ids, user names); and where seal runs in the server,
  seal's wrapping key is derived from it. Dropping it would take a holding
  from every role and show those parameters to whoever reads the database.
  Not decided, and not needed for this plan.
- **A seal permission for public keys alone.** Not needed for intake as
  planned. Needed before the parts served for callers could move.
