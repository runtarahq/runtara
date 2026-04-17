import { test as setup } from '@playwright/test';
import path from 'path';
import { fileURLToPath } from 'url';
import dotenv from 'dotenv';

// Match the env the dev server reads so the OIDC localStorage key we write
// matches the key react-oidc-context reads. Does nothing when env vars are
// already set (e.g. in CI workflow).
dotenv.config();

const __filename = fileURLToPath(import.meta.url);
const __dirnameLocal = path.dirname(__filename);

const authFile = path.join(__dirnameLocal, '.auth/mocked-user.json');

const baseURL = process.env.PLAYWRIGHT_BASE_URL || 'http://localhost:8081';

const MOCKED_AUTHORITY =
  process.env.VITE_OIDC_AUTHORITY || 'https://auth.mocked.test';
const MOCKED_CLIENT_ID = process.env.VITE_OIDC_CLIENT_ID || 'mocked-client';
const MOCKED_AUDIENCE =
  process.env.VITE_OIDC_AUDIENCE || 'https://api.mocked.test';
const MOCKED_ORG_ID = process.env.MOCKED_ORG_ID || 'org_mocked_e2e';

function base64url(input: string): string {
  return Buffer.from(input, 'utf-8')
    .toString('base64')
    .replace(/=+$/, '')
    .replace(/\+/g, '-')
    .replace(/\//g, '_');
}

function buildFakeJwt(payload: Record<string, unknown>): string {
  const header = { alg: 'none', typ: 'JWT' };
  return `${base64url(JSON.stringify(header))}.${base64url(JSON.stringify(payload))}.`;
}

setup('bootstrap mocked auth (no network)', async ({ page }) => {
  const nowSec = Math.floor(Date.now() / 1000);
  const expiresAt = nowSec + 60 * 60;

  const sub = 'auth0|mocked-e2e-user';
  const tokenPayload = {
    sub,
    aud: MOCKED_AUDIENCE,
    iss: `${MOCKED_AUTHORITY}/`,
    iat: nowSec,
    exp: expiresAt,
    org_id: MOCKED_ORG_ID,
    scope: 'openid profile email org_id',
  };

  const accessToken = buildFakeJwt(tokenPayload);
  const idToken = buildFakeJwt({
    ...tokenPayload,
    email: 'e2e@mocked.test',
    email_verified: true,
    name: 'E2E Mocked User',
  });

  const storageKey = `oidc.user:${MOCKED_AUTHORITY}:${MOCKED_CLIENT_ID}`;

  const oidcUser = {
    access_token: accessToken,
    id_token: idToken,
    token_type: 'Bearer',
    scope: 'openid profile email org_id',
    profile: {
      sub,
      aud: MOCKED_CLIENT_ID,
      iss: `${MOCKED_AUTHORITY}/`,
      email: 'e2e@mocked.test',
      name: 'E2E Mocked User',
      org_id: MOCKED_ORG_ID,
    },
    expires_at: expiresAt,
    expires_in: 3600,
    expired: false,
    state: null,
  };

  await page.addInitScript(
    ({ key, value }) => {
      localStorage.setItem(key, JSON.stringify(value));
    },
    { key: storageKey, value: oidcUser }
  );

  // Short-circuit any accidental egress to the (fake) authority.
  await page.route(`${MOCKED_AUTHORITY}/**`, (route) =>
    route.fulfill({ status: 204, body: '' })
  );

  await page.goto(baseURL, { waitUntil: 'domcontentloaded' });

  await page.context().storageState({ path: authFile });
});
