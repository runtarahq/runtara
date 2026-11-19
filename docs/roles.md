# Running the API and the engine as two processes

`runtara-server` does two jobs. It takes requests: the HTTP API, MCP, and
the checks on who is calling. And it is the engine: it runs workflows,
keeps their durable state, compiles them, and uses connections' secrets
through seal. By default one process does both.

`RUNTARA_ROLE` splits them. What takes requests from outside then holds
nothing of the engine's: no runtime database, no key, no way to seal, no
component, no executor certificate. Someone who breaks into it has the
requests that pass through it and the definition tables, and not the runs,
the secrets or the means to start work as a tenant: the engine verifies
for itself who each call is for, and runs only what it approved.

| `RUNTARA_ROLE` | What the process is |
|---|---|
| `all` (default) | Everything, as before. |
| `engine` | The engine alone. It binds no public listener; it listens for the API over mutual TLS. |
| `api` | The API alone. It asks an engine for everything that is the engine's. |
| `intake` | Optional. What receives a trigger's webhooks, in a process that holds no run state. See [Intake](#intake). |

The background to this, and what is still to come, is in
[separating-concerns.md](separating-concerns.md).

## What each process is given

| | API | Engine |
|---|---|---|
| `RUNTARA_SERVER_DATABASE_URL` | yes, as a role of its own | yes |
| `RUNTARA_DATABASE_URL` (the runtime database) | **refused** | yes |
| `RUNTARA_CONNECTIONS_ENCRYPTION_KEY`, `RUNTARA_CONNECTIONS_RETIRED_KEYS` | **refused** | yes |
| seal (`RUNTARA_SEAL_MODE`, `RUNTARA_SEAL_SOCKET`, `RUNTARA_SEAL_ADDRESS`, `RUNTARA_SEAL_TLS_DIR`) | **refused** | required: `unix` or `mtls` ([seal.md](seal.md)) |
| `RUNTARA_SEAL_WRAPPING_KEY` | **refused** | refused, as with any seal of its own |
| `RUNTARA_EXECUTOR_TLS_DIR`, the run channel ([executor.md](executor.md)) | **refused** | as before |
| `RUNTARA_ARTIFACT_STORE_URL` | **refused** | as before |
| `RUNTARA_AGENT_COMPONENTS_DIR` | the agents' `.meta.json` files | the components |
| Valkey (`VALKEY_HOST`, …) | yes, as a user of its own | yes |
| Authentication (`AUTH_PROVIDER`, …), `TENANT_ID`, `RUNTARA_SERVED_TENANTS`, entitlements | yes | yes, the same |
| `SERVER_HOST`, `SERVER_PORT` | yes | unused |
| `RUNTARA_ENGINE_URL` | where the engine listens, `https://host:port` | — |
| `RUNTARA_ENGINE_LISTEN` | **refused** | the address it listens on for the API, `host:port` |
| `RUNTARA_ENGINE_TLS_DIR` | its certificate directory | its certificate directory |
| `RUNTARA_ENGINE_HEALTH_LISTEN` | **refused** | optional: where it says it is up, `host:port` |
| `RUNTARA_COMPILER_WASM` | unused | required without executors: the compiler's WASI command for a sandbox in the engine's process, `runtara-compile-sandboxed.wasm`, which ships beside the components. With executors (`RUNTARA_RUNNER=executor`) it is not loaded: workflows are compiled on one |
| `RUNTARA_COMPILE_GUEST_WASM` | unused | optional, with executors: the compiler that is placed on one, when it is not `runtara-compile-guest.wasm` beside the components |
| `RUNTARA_VALIDATE_GUEST_WASM` | unused | optional, with executors: the save-time validator that is placed on one, when it is not `runtara-validate-placed.wasm` beside the components |
| `RUNTARA_CONVERSATION_QUEUE_BOUND` | unused | optional: how many messages one conversation holds while its run is busy; 32 without it ([native-chat.md](native-chat.md)) |

An API that is given something marked **refused** does not start, and names
what it was given. An engine does not start without
`RUNTARA_ENGINE_LISTEN`, with seal in itself, or without a sandbox to
compile in.

The settings both are given must agree: the same tenants, the same
authentication, the same entitlements, the same Valkey and the same server
database.

## Certificates

The two talk over mutual TLS with certificates from the deployment's own
CA, the one the executors use. `init-pki` makes them all at once:

```sh
runtara-executor init-pki \
  --server-dir <dir for the engine> \
  --executor-dir <dir for the executors> \
  --api-dir <dir for the API>
```

Each directory is given `ca.pem`, `tls.pem` and `tls.key`. The engine's is
its `RUNTARA_ENGINE_TLS_DIR` (and `RUNTARA_EXECUTOR_TLS_DIR`, when it has
executors: it is the same directory, and `RUNTARA_ENGINE_TLS_DIR` may then
be left unset). The API's is its `RUNTARA_ENGINE_TLS_DIR`.

A certificate names what holds it, and each channel takes one name only.
The engine's listener for the API accepts the API's certificate and not an
executor's; the run channel accepts an executor's and not the API's. The
API's certificate is good for calling and not for serving. The CA's key is
written nowhere, so replacing a certificate means running `init-pki
--replace` and handing out all of them again.

## Agents

The API checks the workflows it is given against the agents the engine
runs, and describes them from the `runtara_agent_*.meta.json` files beside
the components. Give it a directory of those files from the same release
the engine runs, as `RUNTARA_AGENT_COMPONENTS_DIR`. It reads the
descriptions and loads no component; the `.wasm` files need not be there.

## Trying it: the evaluation stack

`docker-compose.roles.yml` is an overlay on the evaluation stack that runs
the processes apart: seal, the engine, the API, and intake with a seal of
its own.

```sh
docker compose -f docker-compose.yml -f docker-compose.seal.yml \
  -f docker-compose.roles.yml up
```

The stack's `runtara` service becomes the engine, and a new `api` service
takes the requests on the same port as before. The overlay makes the
certificates, the API's and intake's database roles with their grants, and
the Valkey users, from the files under `scripts/roles/`. It needs
`docker-compose.seal.yml` before it, because the engine starts only with a
seal of its own, and combines with `docker-compose.executor.yml` (before it
too). A stack that has already run without it is brought over as it is.

## Intake

A trigger's webhook, the synchronous trigger and a provider's OAuth
redirect are sent by anyone. A single process receives them itself. An
engine that is a process of its own does not: with `RUNTARA_ROLE=intake` a
third process receives them, so that anonymous traffic does not reach the
process that holds run state.

Intake looks up the trigger a webhook names, checks its signature where the
trigger is bound to a connection that signs, and queues the run. The engine
runs it. For the synchronous trigger it asks the engine to run the workflow
and wait.

| | Intake |
|---|---|
| `RUNTARA_INTAKE_PUBLIC_LISTEN` | the address it receives what anyone may send on itself, `host:port`, in plain HTTP behind whatever terminates TLS |
| `RUNTARA_INTAKE_LISTEN` | the address it listens on for the API, `host:port`, over mutual TLS. One of the two is needed; both may be set |
| `RUNTARA_INTAKE_TLS_DIR` | its certificate directory, from `init-pki --intake-dir` |
| `RUNTARA_ENGINE_URL` | where the engine listens |
| `RUNTARA_INTAKE_HEALTH_LISTEN` | optional: where it answers `/health`, and nothing else |
| `RUNTARA_SERVER_DATABASE_URL` | yes, as a role of its own |
| seal (`RUNTARA_SEAL_MODE`, …) | required, `unix` or `mtls`, as a caller of its own that administers nothing |
| Valkey | yes, as a user of its own |
| `TENANT_ID`, `RUNTARA_SERVED_TENANTS`, entitlements | yes, the same as the engine |
| `RUNTARA_DATABASE_URL`, `RUNTARA_CONNECTIONS_ENCRYPTION_KEY`, `RUNTARA_CONNECTIONS_RETIRED_KEYS`, `RUNTARA_SEAL_WRAPPING_KEY`, `RUNTARA_EXECUTOR_TLS_DIR`, `RUNTARA_ARTIFACT_STORE_URL`, `RUNTARA_ENGINE_LISTEN`, `RUNTARA_COMPILER_WASM` | **refused** |

The API is given `RUNTARA_INTAKE_URL` (`https://host:port`) and calls
intake with the certificate it calls the engine with. An engine that is a
process of its own receives nothing anyone may send: handed a webhook or a
provider's redirect as it arrived, it answers `403 NOT_FOR_THE_ENGINE`. So
an API in front of one, given no `RUNTARA_INTAKE_URL`, has nowhere to hand
those and answers them `503 NO_INTAKE`; everything else it does goes on. A
single process (`RUNTARA_ROLE=all`) receives them itself, as before.

**A listener of its own.** Handed over by the API, what anyone may send
still passes through the process that holds callers' sessions and keys to
the rest of the API. With `RUNTARA_INTAKE_PUBLIC_LISTEN`, intake receives
it itself, and the deployment's edge routes these paths there and
everything else to the API:

| Path | |
|---|---|
| `/api/runtime/events/…` | a trigger's webhooks and the synchronous trigger |
| `/api/runtime/{tenant}/events/…` | the same, in a tenant's own form |
| `/api/oauth/…` | a provider's OAuth redirect |

The listener answers those, `/health`, and `404` to everything else: no
other part of the API is there, with a credential or without. It takes
nothing a request says of itself in the API's words (a caller, a tenant, a
source). The API then needs no `RUNTARA_INTAKE_URL`, and intake no
`RUNTARA_INTAKE_LISTEN`; an API that is still sent one of these paths says
`503 NO_INTAKE`. The evaluation stack's proxy forwards a port and routes no
path, so its overlay has the API hand them over instead.

**What it is granted.** `scripts/roles/intake-database-role.sql` gives its
database role the triggers to read, the tables a queued run is written to,
and nothing else: no connection, no workflow definition, no key, and no
working across tenants. `scripts/roles/intake-valkey-user.acl` lets its
Valkey user count admissions and write product events. Its seal caller is
one that administers nothing and may only verify (`admin = false`,
`destroy_keysets = false`, `operations = ["verify"]`): seal refuses it a
send, a connection or a presigned URL with any value it is handed.

**How a signed webhook is checked without a connection.** What checks a
trigger's webhook is kept on the trigger's row: the connection it is bound
to, the connection's kind, and its signing secret as it is stored, sealed.
The engine writes it when a trigger is saved, and again within thirty
seconds of a connection's secret being entered again. Intake asks seal
whether the signature is that secret's. A trigger bound to a connection
with nothing kept for it yet is refused, not let through.

**What it may ask the engine.** Two things, each what the engine does for
whoever sent the request: to run a workflow and wait, which is what the
synchronous trigger does, and to finish an OAuth flow a provider sent a
user back from. The engine takes those from intake's certificate only, and
nothing else from it.

**A provider's OAuth redirect.** Finishing a flow exchanges a code and
seals tokens, which intake cannot do: it has no connection and no key that
opens one. It receives the redirect, a `GET` under `/api/oauth/`, and hands
the engine what was asked, the path and its query, and the browser's
cookie, which binds the flow to the browser that began it. No other header
and no body crosses. The engine answers the flow in the tenant the redirect
names, as it does for anyone, and intake hands the browser the status, the
`Location`, the cookies and the page it says.

**When something is away.** With the engine away, a webhook is still
accepted and queued, and runs when the engine is back; the synchronous
trigger answers `503`. With intake away, webhooks answer `503` and
everything else the API does goes on.

See [splitting-the-engine.md](splitting-the-engine.md).

`e2e/test_intake.sh` runs seal, an engine, intake and an API as separate
processes and checks all of the above.

## Compiling

A workflow's graph is something a tenant wrote, and compiling it is the
most involved thing the engine does with one. With `RUNTARA_COMPILER_WASM`
the compile step runs in a sandbox of its own: a Wasmtime store with no
network and no environment, a bound on its memory (3 GiB) and its time
(five minutes), and of the filesystem only the component files a compile
reads, read-only. It is given no database, no key and no way to seal. What
it hands back the engine checks: the component against its digest, and the
agents it says it composed against the agents the graph calls.

An engine in a process of its own compiles nowhere else, and refuses to
start without it. A single process may be given it too; without it, it
compiles in its own process as before. The file is
`runtara-compile-sandboxed.wasm`, beside the components in a release
(`agents/` in the bundle, `/opt/runtara/agents/` in the image).

**With executors, a workflow is compiled on one**
([compiler-on-an-executor.md](compiler-on-an-executor.md)), and the engine
does not load the compiler at all. The compiler is placed as a guest, like
a run: in a `runtara-run` that can open no file and make no socket, on a
host with no database. It is given the compile's request and the component
files the request names, which it asks the engine for over the run channel,
and nothing else of the workflow API. So a flaw in Wasmtime itself, which
the sandbox in the engine's process would not contain, lands where there
is nothing to take. There is no compiling anywhere else instead: a compile
no executor takes fails, and a server with executors that has not the
compiler to place (`runtara-compile-guest.wasm`, beside the components)
does not start. What the compiler hands back the engine checks as it does
the sandbox's.

Validating a workflow when it is saved goes the same way: the API asks the
engine, and an engine with executors places the validator on one. With no
executor a workflow is not saved.

Loading the compiler, loading the source, and everything after the build
(the records, the image, what is approved to run) stay the engine's. A
compile in the sandbox makes the same bytes as one in the engine's process.

## Starting and stopping

Start the engine first. It runs the database migrations; an API started
before it runs them itself, and one started with `SKIP_MIGRATIONS=true`
leaves them to the engine.

The engine binds no public listener, so nothing answers `/health` for it
unless it is told where: with `RUNTARA_ENGINE_HEALTH_LISTEN=host:port` it
answers `/health` there once it is ready, and nothing else. It has no TLS,
and is for the engine's own host or container (a container health check, a
process supervisor).

The API starts whether or not the engine answers, and says which in its
log (`Engine at … answers`, later `The engine answers` or `The engine does
not answer`). While the engine is away:

- the API still answers for its own records: workflows and their versions,
  API keys, who the caller is;
- starting a run, and everything else that is the engine's, answers `503`
  with a message that says the engine cannot be reached. Nothing is
  started;
- when the engine is back, the same API process carries on.

Runs do not depend on the API. Stopping or killing it loses none: they are
the engine's, and they complete.

More than one API may run in front of one engine. The engine is one
process, as the server is. A server that runs both roles also listens for
further APIs if it is given `RUNTARA_ENGINE_LISTEN`.

## Who a call is for

The API authenticates a caller for its own routes. What it then asks of the
engine for that caller, it asks with what the caller presented: the token
or API key, the tenant the path named, and which way in it came by (the
API or MCP). The engine verifies that itself, the way the API's middleware
does, with its own keys, its own database and its own membership store. It
does not read whom the API says it authenticated.

- An operation for a tenant is answered only for a caller who acts for
  that tenant. With nothing presented it is refused (`401`), and for
  another tenant's caller too (`403`).
- An operation is held to what the caller's role and API key scope allow,
  whether it changes something (starting or stopping a run, compiling,
  choosing the version that runs) or reads (a run's history, its events,
  the tenant's metrics). Each needs the permission of the API's routes
  that ask it, so the engine refuses nothing the API allows and allows
  nothing the API refuses. The one exception is the list of agents a
  tenant published, which the API shows to every caller.
- A part of the API the engine serves for callers (connections, the object
  model, triggers) is served to the caller the engine found, and
  to nobody without one. A webhook or an OAuth redirect has no caller, and
  the engine does not receive one: intake does, and asks the engine what
  it needs of it as an operation that is intake's alone.
- Whose an API key is, the engine finds. The API's database role cannot
  look across tenants.

So the API's certificate lets it ask, and does not let it say for whom. A
broken API has the requests that pass through it, and can act as their
callers until their tokens expire. A stream that outlasts its token (a
session's events) stops being answered when the token does.

The engine verifies every call, which costs a signature check and, for an
API key, a query; under `RUNTARA_AUTH_MEMBERSHIP_POLICY`, a lookup in
Valkey.

## What makes a workflow runnable

A workflow's definition is a record the API keeps, in the server database.
The engine does not run it because it is there. It keeps its own record,
in the runtime database, of:

- **what was approved**: each version someone who may run it asked to have
  compiled or run, as its definition was when they asked, and the image
  the engine compiled from that;
- **the version that runs** when a run names none, where one was chosen
  (`set-current`). A workflow with none chosen runs its latest version.

A run nobody asked for (a schedule, a webhook, the synchronous HTTP
trigger) goes by those records. If the definition is no longer the one
approved, it is refused, with an error that says so, until a caller who
may compiles or runs it. If the server database names another image than
the engine compiled, the version is compiled again and not run from it. A
current version written to the server database without the engine does
not change what runs.

This holds in one process too. On the first start after an upgrade the
engine carries over what was already compiled and chosen, once.

## The database roles and the Valkey users

The two processes are given roles of their own. The files are in
`scripts/roles/`.

**Postgres.** The engine's role owns both databases and runs the
migrations. The API's role owns nothing and starts with
`SKIP_MIGRATIONS=true`. Both are plain login roles (`NOSUPERUSER
NOBYPASSRLS`), and both set `RUNTARA_REQUIRE_ROW_SECURITY=true`. Once the
engine has run the migrations, and again after an upgrade that adds a
table:

```sh
psql -d <server database> -v api_role=<role> -v engine_role=<role> \
     -f scripts/roles/api-database-role.sql
```

It creates `runtara_system`, the role whose members may work across
tenants, and grants it to the engine's role and not to the API's. It
grants the API's role:

| | Tables |
|---|---|
| read and write | `workflows`, `workflow_definitions`, `operations_views`, `api_keys` |
| read and add | `audit_events` |
| read and update | `invocation_trigger` (a deleted workflow's triggers are switched off) |
| read | `workflow_compilations`, `workflow_dependencies`, `workflow_metrics_hourly`, `workflow_metrics_daily` |
| nothing | the outbox and admission tables, connections, OAuth state and apps, `seal_wrapped_key`, the object model's, everything in the runtime database |

`runtara_system` is a role of the cluster: every server on the same
Postgres cluster is then held to it, so each one's engine role is granted
it.

**Valkey.** The API is given a user (`VALKEY_USER`, `VALKEY_PASSWORD`)
with the rules in `scripts/roles/api-valkey-user.acl`:

```sh
valkey-cli ACL SETUSER runtara-api on '><password>' \
    $(grep -v '^#' scripts/roles/api-valkey-user.acl)
```

It reads who is a member and which tokens are revoked, counts its tenants'
quotas, keeps MCP sessions, writes product events and
reads the control stream. It cannot write a trigger stream, a compilation
queue, an admission count or a membership, or add to the control stream.
The engine's user keeps every key. Turn the `default` user off, or give it
a password the API does not have.

Calls have a deadline: a minute, or fifteen for one that is held until
work is done (running a workflow and waiting for it, compiling, running an
agent, and any request the engine serves, such as a synchronous HTTP
trigger).

## Checking it

`e2e/test_two_roles.sh` starts seal, an engine, two executors and an API
as separate processes and runs through the above. `e2e/test_two_roles_oidc.sh`
runs the two with OIDC sign-in and membership required, and asks the engine
directly with tokens that are forged, expired, another tenant's, revoked, a
non-member's, a viewer's, and for the other way in. `TWO_ROLES=true
e2e/test_served_tenants.sh` runs the two-tenant test the same way, with
the API on the database role and Valkey user above. Its last section turns
what the API holds against the engine: its certificate asks for another
tenant, its database role reaches for the outbox, the keys and the runtime
database, its Valkey user writes a trigger stream, and a definition and a
current version are changed in the server database behind the engine.

## A hosted deployment

The same three processes map onto hosts:

- **The proxy in front** routes public traffic to the API processes and to
  nothing else. The engine's listener for the API, the run channel, the
  executors and seal are on ports it does not route and, where it can be
  arranged, on hosts it does not share.
- **API processes** scale out behind it. Each is given the API's
  certificate, the API's database role and the API's Valkey user, and none
  of what the table above marks refused.
- **The engine** is one process, on a host the proxy does not reach, with
  both databases, the engine's Valkey user and its certificate. Its health
  is `RUNTARA_ENGINE_HEALTH_LISTEN`.
- **seal** is on a host of its own where it can be, with the keys
  ([seal.md](seal.md)); executors are on theirs ([executor.md](executor.md)).
- **Postgres** takes the engine's role for both databases and the API's for
  the server database only: refuse the API's role `CONNECT` on the runtime
  database. **Valkey** has the `default` user off, and a user for each.
