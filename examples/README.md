# Runtara example workflows

A small set of workflows showcasing core control-flow features. They are
**not** part of the server image — they are seeded into a running instance on
demand by the evaluation stack (`docker-compose.yml` at the repo root), or by
running `seed.py` directly.

| File | Feature | Try it with |
| --- | --- | --- |
| `workflows/01-conditional.json` | `Conditional` step, `true`/`false` branch edges | `{ "amount": 150 }` |
| `workflows/02-error-handling.json` | `onError` edge — recover instead of failing | `{}` |
| `workflows/03-split.json` | `Split` — iterate an array, collect per-item results | `{ "items": ["a","b","c"] }` |
| `workflows/04-while.json` | `While` — loop on `loop.index` with a `maxIterations` cap | `{ "target": 3 }` |
| `workflows/05-api-call.json` | `Agent` (HTTP) — call a public REST API | `{}` |

`index.json` is the manifest `seed.py` reads to find the workflow files.

## Seeding

### Via the evaluation stack (opt-in)

The root `docker-compose.yml` includes a `seeder` service that runs only when
`FETCH_EXAMPLES=yes`. It fetches `seed.py` + these workflow files from GitHub
and installs them into the running server:

```bash
FETCH_EXAMPLES=yes docker compose up
```

Pin a specific ref with `EXAMPLES_REF` (default `main`).

### Manually

`seed.py` is stdlib-only (no pip install). Point it at a running server and
either a local copy of this directory or a base URL:

```bash
# from a local checkout
EXAMPLES_DIR=./examples RUNTARA_API_BASE=http://127.0.0.1:7001 python3 examples/seed.py

# or fetch from a URL
EXAMPLES_BASE_URL=https://raw.githubusercontent.com/runtarahq/runtara/main/examples \
  RUNTARA_API_BASE=http://127.0.0.1:7001 python3 examples/seed.py
```

For each workflow `seed.py` does: create → store graph → compile (blocking) →
set current version. It is idempotent — a workflow whose name already exists is
skipped.

## Adding an example

1. Add `workflows/NN-name.json` (a workflow graph: `name`, `description`,
   `entryPoint`, `steps`, `executionPlan`; declare an `inputSchema` for any
   `data.*` references).
2. Add the filename to `index.json`.

See `get_workflow_authoring_schema` (MCP) for the canonical graph shapes.
