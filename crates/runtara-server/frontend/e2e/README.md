# E2E Testing

End-to-end tests live in this directory and run with Playwright.

## Projects

`playwright.config.ts` defines four projects with different trust boundaries:

| Project    | Test files                                                     | Network                                                     | Use when                                                                                                                   |
| ---------- | -------------------------------------------------------------- | ----------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| `mocked`   | `tests/mocked/**/*.mocked.spec.ts`                             | **None** — every API call is intercepted via `page.route()` | **PR gate.** Fast, deterministic, no secrets required. Fork PRs can run this.                                              |
| `smoke`    | `tests/smoke/**/*.smoke.spec.ts`                               | Real API — prod or staging                                  | Daily monitoring of the deployed app. CI runs in local auth mode, so no Auth0 secrets are required.                        |
| `e2e`      | `tests/e2e/**/*.e2e.spec.ts`                                   | Real API + full local backend stack                         | Deep end-to-end flows that exercise the runtime (gateway + DB + scheduler). Requires the backend services running locally. |
| Root tests | `tests/*.spec.ts` (auth, login, navigation, components, theme) | Real API (uses `user.json` state)                           | Browser-facing checks that don't fit neatly into smoke or e2e.                                                             |

Each project has its own auth bootstrap:

- `auth.setup.ts` — Auth0 Client-Credentials Grant; writes real token to `.auth/user.json`. Used by `setup`, `chromium`, `e2e`, and `smoke` only when `E2E_SMOKE_AUTH_MODE` / `VITE_RUNTARA_AUTH_MODE` is not `local` or `trust_proxy`.
- `auth.mocked.setup.ts` — synthesizes a structurally valid but unsigned JWT and writes it to `.auth/mocked-user.json`. No network calls. Used by `mocked`.

## Running

```bash
# PR gate suite (fast, no secrets needed if env placeholders are set)
npm run test:e2e:mocked
npm run test:e2e:mocked:headed       # watch in a browser
npm run test:e2e:mocked:update       # regenerate visual snapshots

# Real-API smoke. Set E2E_SMOKE_AUTH_MODE=local when the target accepts local/trust-proxy auth.
npm run test:e2e:smoke

# Full-stack e2e (needs local gateway + backend stack)
npm run test:e2e:local

# All projects (the original top-level entrypoint)
npm run test:e2e
```

## Writing a new mocked spec

Three conventions:

1. **Use a Page Object** under `e2e/pages/`. One class per route; it exposes actions and locators. Keep assertions in the spec, not the PO.
2. **Call `mockApi.bootstrap(page)` first** so the sidebar and health check have data. Then add page-specific mocks.
3. **End with `runA11y(page)`** and optionally `view.expectMatchesSnapshot('name')` for visual regression.

Mock URL patterns are built by `runtimeUrl()` in `fixtures/mock.fixture.ts` — it handles the optional org_id prefix that the axios interceptor adds and only matches exact paths (not subpaths) so `runtimeUrl('workflows')` does not accidentally swallow `workflows/folders`.

Example:

```ts
import { test, buildSchema } from '../../../fixtures';
import { ObjectSchemasPage } from '../../../pages/ObjectSchemasPage';

test('renders list', async ({ page, mockApi, runA11y }) => {
  await mockApi.bootstrap(page);
  await mockApi.objects.schemas.list(page, [
    buildSchema({ name: 'Customers' }),
  ]);

  const view = new ObjectSchemasPage(page);
  await view.goto();
  await view.expectHeading(/object types/i);
  await runA11y(page);
  await view.expectMatchesSnapshot('objects-schemas-list');
});
```

## Accessibility checks

`runA11y(page)` runs axe-core against the current page and attaches the full JSON report (`axe-results.json`) to the Playwright test attachment, regardless of pass/fail.

By default it **fails on `critical` violations only**. Set `E2E_A11Y_STRICT=true` to also fail on `serious`.

### Rules currently suppressed by default

`fixtures/a11y.fixture.ts` keeps a list of rule IDs where the app has pre-existing violations. Each one is **still captured in the report** — the gate just doesn't block. Fix these incrementally and remove the corresponding rule ID from the list:

- `button-name` — icon-only buttons missing `aria-label`
- `color-contrast` — muted text below AA threshold
- `list` / `listitem` — sidebar nests components inside `<ul>`
- `aria-allowed-attr`, `aria-required-children`, `aria-required-parent` — Radix UI primitives
- `select-name` — unlabeled selects in filter toolbars

**Do not add to this list without tracking the violation in the backlog.**

## Visual regression

`view.expectMatchesSnapshot('name')` calls `toHaveScreenshot()` under the hood, but **only when `E2E_VISUAL=true`**. Snapshots are platform-specific (darwin vs linux font rendering differs), so we generate and compare them on a canonical platform — the CI ubuntu runners.

Workflow:

1. Land a PR that adds specs without snapshots. The `mocked` gate passes because `E2E_VISUAL` is unset.
2. To enable visual regression for a given spec, set `E2E_VISUAL=true` in the CI workflow and run `--update-snapshots` once; commit the generated `*.png` files under `__snapshots__/` directories. Subsequent runs enforce them.
3. Locally, to regenerate: `E2E_VISUAL=true npm run test:e2e:mocked:update` (but only commit the linux version — run in the docker-compose image for an accurate match).

## Full-stack seeding

`utils/seed.ts` wraps API calls that create fixtures for `*.e2e.spec.ts` tests:

```ts
import { seededApi, seedWorkflow, cleanupAllSeeded } from '../../utils/seed';

test.beforeAll(async () => {
  api = await seededApi();
  workflow = await seedWorkflow(api, { scope: 'my-suite', name: 'My test' });
});

test.afterAll(async () => {
  await cleanupAllSeeded(api, 'my-suite');
  await api.dispose();
});
```

The token is read from `.auth/user.json` (set by `auth.setup.ts`). Seeded entities include a tag `[__e2e:<scope>:<run-id>]` in their name/description so `cleanupAllSeeded` can reap them even after a crashed test — don't rely on `afterEach` for cleanup alone.

## CI

- `.github/workflows/pr-checks.yml` — runs `lint`, `typecheck`, `unit`, `build`, `e2e-mocked` on every PR against `main`. All five jobs must pass. **The user needs to mark these as required status checks in GitHub branch-protection settings** (UI action, not a repo file).
- `.github/workflows/smoke-tests.yml` — push to `main`, scheduled daily, and manual dispatch. Runs the `smoke` project against a real API in local auth mode. Not a PR gate.

## Directory layout

```
e2e/
├── .auth/                      # generated auth state (gitignored)
├── auth.setup.ts               # real Auth0 setup (for chromium, smoke, e2e)
├── auth.mocked.setup.ts        # fake token setup (for mocked)
├── pages/                      # page object classes
├── fixtures/
│   ├── auth.fixture.ts         # isAuthenticated/clearAuth helpers
│   ├── api.fixture.ts          # (legacy) generic mock helpers
│   ├── mock.fixture.ts         # typed, high-level API mocks
│   ├── a11y.fixture.ts         # runA11y() wrapping @axe-core/playwright
│   ├── builders.ts             # typed fixture factories (buildWorkflow, etc.)
│   └── index.ts                # merged test + expect entrypoint
├── utils/
│   ├── auth-token.ts           # Auth0 client-credentials flow helper
│   ├── test-helpers.ts         # reusable interaction primitives
│   └── seed.ts                 # full-stack seeding helpers
└── tests/
    ├── *.spec.ts               # root-level navigation/auth/theme
    ├── smoke/*.smoke.spec.ts   # real-API smoke
    ├── e2e/*.e2e.spec.ts       # full-stack
    └── mocked/**/*.mocked.spec.ts
```
