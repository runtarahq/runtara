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
| `all` (default) | Both, as before. |
| `engine` | The engine alone. It binds no public listener; it listens for the API over mutual TLS. |
| `api` | The API alone. It asks an engine for everything that is the engine's. |

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

An API that is given something marked **refused** does not start, and names
what it was given. An engine does not start without
`RUNTARA_ENGINE_LISTEN`, or with seal in itself.

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

## Starting and stopping

Start the engine first. It runs the database migrations; an API started
before it runs them itself, and one started with `SKIP_MIGRATIONS=true`
leaves them to the engine.

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
- An operation that changes something (starting or stopping a run,
  compiling, choosing the version that runs) is held to what the caller's
  role and API key scope allow, by the permissions the API's routes use.
  Reading is open to any member of the tenant.
- A part of the API the engine serves for callers (connections, the object
  model, agents, triggers) is served to the caller the engine found, and
  to nobody without one. A webhook or an OAuth redirect has no caller, and
  is checked by the engine as it is when anyone sends one.
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
quotas, keeps MCP sessions and session queues, writes product events and
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
as separate processes and runs through the above. `TWO_ROLES=true
e2e/test_served_tenants.sh` runs the two-tenant test the same way, with
the API on the database role and Valkey user above. Its last section turns
what the API holds against the engine: its certificate asks for another
tenant, its database role reaches for the outbox, the keys and the runtime
database, its Valkey user writes a trigger stream, and a definition and a
current version are changed in the server database behind the engine.
