import path from 'path';
import { fileURLToPath } from 'url';
import dotenv from 'dotenv';

const __filename = fileURLToPath(import.meta.url);
const __dirnameLocal = path.dirname(__filename);

// Load .env file from project root
dotenv.config({ path: path.resolve(__dirnameLocal, '../../.env') });

export interface Auth0TokenResponse {
  access_token: string;
  id_token?: string;
  token_type: string;
  expires_in: number;
  scope?: string;
}

export async function getAuth0Token(): Promise<Auth0TokenResponse> {
  const domain = process.env.VITE_OIDC_AUTHORITY?.replace('https://', '');
  const clientId = process.env.TEST_APP_CLIENT_ID;
  const clientSecret = process.env.TEST_APP_CLIENT_SECRET;
  const audience = process.env.VITE_OIDC_AUDIENCE;
  const orgId = process.env.TEST_ORG_ID;

  if (!domain) {
    throw new Error('Missing VITE_OIDC_AUTHORITY environment variable');
  }
  if (!clientId) {
    throw new Error('Missing TEST_APP_CLIENT_ID environment variable');
  }
  if (!clientSecret) {
    throw new Error('Missing TEST_APP_CLIENT_SECRET environment variable');
  }
  if (!orgId) {
    throw new Error('Missing TEST_ORG_ID environment variable');
  }

  const tokenUrl = `https://${domain}/oauth/token`;

  const tokenRequestBody: Record<string, string> = {
    grant_type: 'client_credentials',
    client_id: clientId,
    client_secret: clientSecret,
    organization: orgId,
  };

  if (audience) {
    tokenRequestBody.audience = audience;
  }

  const response = await fetch(tokenUrl, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(tokenRequestBody),
  });

  if (!response.ok) {
    const errorBody = await response.text();
    throw new Error(
      `Auth0 token request failed: ${response.status} ${response.statusText}\n${errorBody}`
    );
  }

  return response.json();
}
