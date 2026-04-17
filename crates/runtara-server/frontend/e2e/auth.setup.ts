import { test as setup } from '@playwright/test';
import path from 'path';
import { fileURLToPath } from 'url';
import { getAuth0Token } from './utils/auth-token';

// ES module equivalent of __dirname
const __filename = fileURLToPath(import.meta.url);
const __dirnameLocal = path.dirname(__filename);

/**
 * Auth0 Authentication Setup using Client Credentials
 *
 * This file runs ONCE before all tests to authenticate with Auth0
 * using the Client Credentials Grant and save the session state for reuse.
 *
 * Required environment variables:
 * - TEST_APP_CLIENT_ID: Auth0 application client ID for testing
 * - TEST_APP_CLIENT_SECRET: Auth0 application client secret
 * - TEST_ORG_ID: Auth0 organization ID
 * - VITE_OIDC_AUTHORITY: OIDC issuer URL (e.g., "https://your-tenant.auth0.com")
 * - VITE_OIDC_AUDIENCE: Auth0 API audience
 * - VITE_OIDC_CLIENT_ID: The app's own client ID (used for localStorage key)
 */

const authFile = path.join(__dirnameLocal, '.auth/user.json');

// Base URL for the app - can be overridden via environment variable
const baseURL = process.env.PLAYWRIGHT_BASE_URL || 'http://localhost:8081';

setup('authenticate with Auth0', async ({ page }) => {
  const domain = process.env.VITE_OIDC_AUTHORITY?.replace('https://', '');
  // Use the app's client ID for the localStorage key (not the test client ID)
  const appClientId = process.env.VITE_OIDC_CLIENT_ID;

  if (!appClientId) {
    throw new Error('Missing VITE_OIDC_CLIENT_ID environment variable');
  }

  console.log('Fetching Auth0 token using Client Credentials Grant...');

  const tokenResponse = await getAuth0Token();

  console.log('Token obtained successfully (includes org_id).');

  // Inject the OIDC tokens into localStorage
  // The oidc-client-ts library uses a specific key format: oidc.user:{authority}:{client_id}
  const authority = `https://${domain}`;
  const storageKey = `oidc.user:${authority}:${appClientId}`;

  // Calculate token expiration
  const expiresAt = Math.floor(Date.now() / 1000) + tokenResponse.expires_in;

  // Decode the access token to get the sub claim
  const tokenPayload = JSON.parse(
    Buffer.from(tokenResponse.access_token.split('.')[1], 'base64').toString()
  );

  // Create the OIDC user object that oidc-client-ts expects
  const oidcUser = {
    access_token: tokenResponse.access_token,
    id_token: tokenResponse.id_token || '',
    token_type: tokenResponse.token_type,
    scope: tokenResponse.scope || 'openid email phone',
    profile: {
      sub: tokenPayload.sub,
      aud: appClientId,
      iss: `${authority}/`,
    },
    expires_at: expiresAt,
    expires_in: tokenResponse.expires_in,
    expired: false,
    state: null,
  };

  // Use addInitScript to inject token into localStorage BEFORE page loads
  // This prevents the app from redirecting to Auth0 login
  await page.addInitScript(
    ({ key, value }) => {
      localStorage.setItem(key, JSON.stringify(value));
    },
    { key: storageKey, value: oidcUser }
  );

  console.log(`OIDC token will be injected with key: ${storageKey}`);

  // Now navigate to the app - token is already in localStorage
  await page.goto(baseURL, { waitUntil: 'networkidle' });

  // Verify authentication by checking we're not redirected to login
  const currentUrl = page.url();
  if (currentUrl.includes('login') || currentUrl.includes('authorize')) {
    throw new Error(
      `Authentication failed - still on login page: ${currentUrl}`
    );
  }

  console.log('Authentication verified. Current URL:', currentUrl);

  // Save the authenticated state including localStorage
  await page.context().storageState({ path: authFile });

  console.log('Authentication successful! State saved to:', authFile);
});
