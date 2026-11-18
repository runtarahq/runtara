# Running the API and the engine as two processes

`runtara-server` does two jobs. It takes requests: the HTTP API, MCP, and
the checks on who is calling. And it is the engine: it runs workflows,
keeps their durable state, compiles them, and uses connections' secrets
through seal. By default one process does both.

`RUNTARA_ROLE` splits them. What takes requests from outside then holds
nothing of the engine's: no runtime database, no key, no way to seal, no
component, no executor certificate. Someone who breaks into it has the
requests that pass through it and the definition tables, and not the runs,
the secrets or the means to start work as a tenant.

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
| `RUNTARA_SERVER_DATABASE_URL` | yes | yes |
| `RUNTARA_DATABASE_URL` (the runtime database) | **refused** | yes |
| `RUNTARA_CONNECTIONS_ENCRYPTION_KEY`, `RUNTARA_CONNECTIONS_RETIRED_KEYS` | **refused** | yes |
| seal (`RUNTARA_SEAL_MODE`, `RUNTARA_SEAL_SOCKET`, `RUNTARA_SEAL_ADDRESS`, `RUNTARA_SEAL_TLS_DIR`) | **refused** | required: `unix` or `mtls` ([seal.md](seal.md)) |
| `RUNTARA_SEAL_WRAPPING_KEY` | **refused** | refused, as with any seal of its own |
| `RUNTARA_EXECUTOR_TLS_DIR`, the run channel ([executor.md](executor.md)) | **refused** | as before |
| `RUNTARA_ARTIFACT_STORE_URL` | **refused** | as before |
| `RUNTARA_AGENT_COMPONENTS_DIR` | the agents' `.meta.json` files | the components |
| Valkey (`VALKEY_HOST`, …) | yes | yes |
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

## What crosses between them

The API authenticates the caller, and tells the engine who that is with
each call. The engine believes it: the API's certificate is what the
engine trusts, as the code in one process trusted itself. A broken API can
therefore still say "this is tenant X" to the engine. Closing that (the
engine checks the caller's token itself, and the API's database role is
held to definition tables) is the next phase in
[separating-concerns.md](separating-concerns.md).

Calls have a deadline: a minute, or fifteen for one that is held until
work is done (running a workflow and waiting for it, compiling, running an
agent, and any request the engine serves, such as a synchronous HTTP
trigger).

## Checking it

`e2e/test_two_roles.sh` starts seal, an engine, two executors and an API
as separate processes and runs through the above. `TWO_ROLES=true
e2e/test_served_tenants.sh` runs the two-tenant test the same way, on
database roles bound by row-level security.
