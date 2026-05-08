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
