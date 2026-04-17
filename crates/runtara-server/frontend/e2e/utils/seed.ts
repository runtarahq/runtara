import {
  APIRequestContext,
  request as playwrightRequest,
} from '@playwright/test';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirnameLocal = path.dirname(__filename);

const authFile = path.join(__dirnameLocal, '../.auth/user.json');
const API_BASE =
  process.env.VITE_RUNTARA_API_BASE_URL ||
  process.env.PLAYWRIGHT_API_BASE_URL ||
  'http://localhost:7001';

export const TEST_RUN_ID = `e2e-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`;

/** Marker appended to seeded-entity names so cleanupAllSeeded can find them even after a crashed run. */
export const SEED_TAG = (scope: string): string =>
  `[__e2e:${scope}:${TEST_RUN_ID}]`;

function readAccessToken(): string {
  if (process.env.E2E_ACCESS_TOKEN) return process.env.E2E_ACCESS_TOKEN;
  const state = JSON.parse(fs.readFileSync(authFile, 'utf8'));
  for (const origin of state.origins ?? []) {
    for (const entry of origin.localStorage ?? []) {
      if (
        typeof entry.name === 'string' &&
        entry.name.startsWith('oidc.user:')
      ) {
        const user = JSON.parse(entry.value);
        if (user.access_token) return user.access_token;
      }
    }
  }
  throw new Error(
    `Could not find an access_token in ${authFile}. Run \`npx playwright test --project=setup\` first.`
  );
}

/**
 * Seeded request context pre-populated with the auth token from auth.setup.ts.
 * Use this to create fixtures over HTTP before a test hits the UI.
 *
 * Example:
 *   test.beforeAll(async () => {
 *     api = await seededApi();
 *     scenario = await seedScenario(api, { scope: 'scenario-lifecycle' });
 *   });
 *   test.afterAll(async () => {
 *     await cleanupAllSeeded(api, 'scenario-lifecycle');
 *     await api.dispose();
 *   });
 */
export async function seededApi(): Promise<APIRequestContext> {
  const token = readAccessToken();
  return playwrightRequest.newContext({
    baseURL: API_BASE,
    extraHTTPHeaders: {
      Authorization: `Bearer ${token}`,
    },
  });
}

async function orgPrefix(api: APIRequestContext): Promise<string> {
  if (process.env.VITE_STRIP_ORG_ID === 'true') return '';
  if (process.env.TEST_ORG_ID) return `/${process.env.TEST_ORG_ID}`;
  // Extract from token as last resort
  const token = (await api.storageState()).origins
    .flatMap((o) => o.localStorage)
    .find((l) => l.name.startsWith('oidc.user:'))?.value;
  if (!token) return '';
  try {
    const user = JSON.parse(token);
    const payload = JSON.parse(
      Buffer.from(user.access_token.split('.')[1], 'base64').toString()
    );
    return payload.org_id ? `/${payload.org_id}` : '';
  } catch {
    return '';
  }
}

async function runtimePath(
  api: APIRequestContext,
  suffix: string
): Promise<string> {
  const org = await orgPrefix(api);
  return `/api/runtime${org}/${suffix.replace(/^\//, '')}`;
}

export interface SeedScenarioOptions {
  scope: string;
  name?: string;
  description?: string;
}

export async function seedScenario(
  api: APIRequestContext,
  opts: SeedScenarioOptions
): Promise<{ id: string; name: string }> {
  const name = opts.name ?? `Seed Scenario ${SEED_TAG(opts.scope)}`;
  const response = await api.post(await runtimePath(api, 'scenarios'), {
    data: {
      name,
      description: opts.description ?? `Seeded by e2e. ${SEED_TAG(opts.scope)}`,
    },
  });
  if (!response.ok()) {
    throw new Error(
      `seedScenario failed: ${response.status()} ${await response.text()}`
    );
  }
  const body = await response.json();
  const id = body.data?.id ?? body.id;
  if (!id)
    throw new Error(
      `seedScenario response missing id: ${JSON.stringify(body)}`
    );
  return { id, name };
}

export interface SeedConnectionOptions {
  scope: string;
  integrationId: string;
  title?: string;
  parameters?: Record<string, unknown>;
}

export async function seedConnection(
  api: APIRequestContext,
  opts: SeedConnectionOptions
): Promise<{ id: string; title: string }> {
  const title =
    opts.title ?? `Seed ${opts.integrationId} ${SEED_TAG(opts.scope)}`;
  const response = await api.post(await runtimePath(api, 'connections'), {
    data: {
      title,
      integrationId: opts.integrationId,
      parameters: opts.parameters ?? {},
    },
  });
  if (!response.ok()) {
    throw new Error(
      `seedConnection failed: ${response.status()} ${await response.text()}`
    );
  }
  const body = await response.json();
  const id = body.connection?.id ?? body.id;
  if (!id)
    throw new Error(
      `seedConnection response missing id: ${JSON.stringify(body)}`
    );
  return { id, title };
}

/**
 * Delete every scenario and connection whose name or description contains the
 * given scope tag. Safe to call even if the test crashed — it only touches
 * entities this suite created.
 */
export async function cleanupAllSeeded(
  api: APIRequestContext,
  scope: string
): Promise<void> {
  const tag = SEED_TAG(scope);

  const connectionsResp = await api.get(await runtimePath(api, 'connections'));
  if (connectionsResp.ok()) {
    const body = await connectionsResp.json();
    for (const conn of body.connections ?? []) {
      if (typeof conn.title === 'string' && conn.title.includes(tag)) {
        await api.delete(await runtimePath(api, `connections/${conn.id}`));
      }
    }
  }

  const scenariosResp = await api.get(
    await runtimePath(api, 'scenarios?pageSize=100&recursive=true')
  );
  if (scenariosResp.ok()) {
    const body = await scenariosResp.json();
    for (const scn of body.data?.content ?? []) {
      const match =
        (typeof scn.name === 'string' && scn.name.includes(tag)) ||
        (typeof scn.description === 'string' && scn.description.includes(tag));
      if (match) {
        await api.delete(await runtimePath(api, `scenarios/${scn.id}`));
      }
    }
  }
}
