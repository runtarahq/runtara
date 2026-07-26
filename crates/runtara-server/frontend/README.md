# Runtara Frontend

React + Vite + TypeScript SPA for the Runtara runtime. Built to be embedded
into the `runtara-server` binary (via [`rust_embed`](../Cargo.toml)) and
served at a runtime-configurable mount prefix.

## Quick start

```bash
npm install        # Node version pinned in `.node-version`
cp .env.example .env
# fill in VITE_RUNTARA_API_BASE_URL and the VITE_OIDC_* values
npm run dev        # http://localhost:8081
```

## Building for embed

```bash
npm run build      # writes ./dist/
```

That's what `cargo build -p runtara-server --features embed-ui` bundles.
`npm run build` generates `src/wasm/validation/` first. That directory
is ignored by Git and should not be edited or committed. During an embedded
Cargo build, the server build script also regenerates it if the shared Rust
validator changed. A running Vite dev/watch process will pick up those generated
file changes.

## Dev loops

Pick by what you're changing. The thing to avoid is the fourth combination —
`embed-ui` plus a running watcher — which costs a full Rust recompile and a
~113MB relink per keystroke.

**Frontend only** → `npm run dev` (:8081). HMR, no Rust in the loop. Needs
`.env.local` for the API base and auth mode.

**Frontend against the real server surface** — mount prefix, injected
`<base href>`, CSP, single origin, no CORS — → run the server with
`RUNTARA_UI_DIST_DIR` and keep a watcher going:

```bash
RUNTARA_UI_DIST_DIR=$PWD/crates/runtara-server/frontend/dist cargo run -p runtara-server --release
```

```bash
cd crates/runtara-server/frontend && npm run build:watch
```

The server reads `dist/` per request, so a save plus a browser reload is the
whole loop — no cargo build, no restart. It works in any profile and takes
precedence over an embedded copy, so `--features embed-ui` is unnecessary here
(and leaving it off is what keeps the watcher from invalidating your Rust
builds — see below). `build:watch` skips `prebuild`, so run
`npm run build:wasm-validation` once if `src/wasm/validation/` is missing.

**Rust only** → build without `embed-ui`. `build.rs` declares
`rerun-if-changed=frontend/dist` only under that feature, so without it a
running watcher can't touch your build cache. With it, every `dist` write reruns
the build script, which recompiles `runtara-server` and relinks the binary — and
if cargo catches `dist` mid-rebuild, the build fails outright on a missing
`index.html`.

**Shipping / verifying the real embedded bundle** → stop the watcher, then
`npm run build && cargo build -p runtara-server --release --features embed-ui`.
That's the only combination that needs the relink, and it should be a deliberate
one-shot rather than something in your edit loop.

## Runtime configuration

The bundle is **mount-agnostic and tenant-agnostic**: one build deploys
anywhere. At startup, `runtara-server` injects `<base href>` and
`window.__RUNTARA_CONFIG__` into `index.html` from `RUNTARA_UI_*` env vars:

| Runtime env (`RUNTARA_UI_*`)  | Build-time fallback (`VITE_*`)                 | Consumer                |
| ----------------------------- | ---------------------------------------------- | ----------------------- |
| `RUNTARA_UI_OIDC_AUTHORITY`   | `VITE_OIDC_AUTHORITY`                          | OIDC                    |
| `RUNTARA_UI_OIDC_CLIENT_ID`   | `VITE_OIDC_CLIENT_ID`                          | OIDC                    |
| `RUNTARA_UI_OIDC_AUDIENCE`    | `VITE_OIDC_AUDIENCE`                           | OIDC                    |
| `RUNTARA_UI_API_BASE_URL`     | `VITE_RUNTARA_API_BASE_URL`                    | API client              |
| `RUNTARA_UI_PLAUSIBLE_DOMAIN` | `VITE_RUNTARA_PLAUSIBLE_DOMAIN`                | Analytics (opt-in)      |
| `RUNTARA_UI_PLAUSIBLE_HOST`   | `VITE_RUNTARA_PLAUSIBLE_HOST`                  | Analytics (opt-in)      |
| server build stamp            | `VITE_RUNTARA_VERSION` / `VITE_RUNTARA_COMMIT` | Sidebar version display |

Config resolution lives in [`src/shared/config/runtimeConfig.ts`](src/shared/config/runtimeConfig.ts).
Vite dev server and tests use the build-time fallbacks; the embedded
server overrides them.

## Scripts

- `npm run dev` — Vite dev server on `:8081`
- `npm run build` — TypeScript check + production bundle into `dist/`
- `npm run preview` — serve the production bundle locally
- `npm run lint` — ESLint
- `npm test` — Vitest
- `npm run test:e2e` — Playwright (see `e2e/README.md`)
- `npm run generate-api-runtime-local` / `generate-api-management-local` — regenerate TypeScript API clients from live OpenAPI specs

## Project layout

```
src/
├── features/   # domain modules (workflows, connections, objects, triggers, …)
├── shared/     # cross-feature UI, hooks, stores, queries, config
├── generated/  # swagger-typescript-api output — do not hand-edit
├── router/     # React Router definitions
└── test/       # vitest setup & utilities
```

## License

AGPL-3.0-or-later. See [`LICENSING.md`](LICENSING.md).
