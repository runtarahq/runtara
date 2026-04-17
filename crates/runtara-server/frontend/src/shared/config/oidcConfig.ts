import { WebStorageStateStore } from 'oidc-client-ts';
import { config } from '@/shared/config/runtimeConfig';

const onSigninCallback = () => {
  window.history.replaceState({}, document.title, window.location.pathname);
};

const OIDC_AUTHORITY = config.oidc.authority;
const OIDC_CLIENT_ID = config.oidc.clientId;
const OIDC_AUDIENCE = config.oidc.audience;

if (!OIDC_AUTHORITY || !OIDC_CLIENT_ID || !OIDC_AUDIENCE) {
  throw new Error(
    'Missing required OIDC configuration. Provide either window.__RUNTARA_CONFIG__.oidc{Authority,ClientId,Audience} (injected by the server from RUNTARA_UI_OIDC_* env vars) or VITE_OIDC_AUTHORITY / VITE_OIDC_CLIENT_ID / VITE_OIDC_AUDIENCE at build time.'
  );
}

// Full absolute URL of the SPA root (protocol + host + mount path), e.g.
// `http://localhost:7001/ui/org_abc/`. `document.baseURI` reflects the
// `<base href>` the server injects at startup, so Auth0 redirects land
// back on the tenant-scoped mount instead of the bare origin.
const appBaseUrl = document.baseURI;

export const oidcConfig = {
  authority: OIDC_AUTHORITY,
  client_id: OIDC_CLIENT_ID,
  response_type: 'code',
  scope: 'email openid phone org_id',
  redirect_uri: appBaseUrl,
  post_logout_redirect_uri: appBaseUrl,
  extraTokenParams: { audience: OIDC_AUDIENCE },
  extraQueryParams: { audience: OIDC_AUDIENCE },
  automaticSilentRenew: true,
  userStore: new WebStorageStateStore({ store: window.localStorage }),
  onSigninCallback,
  // Override metadata to use Auth0's /v2/logout endpoint which supports returnTo param
  metadata: {
    issuer: `${OIDC_AUTHORITY}/`,
    authorization_endpoint: `${OIDC_AUTHORITY}/authorize`,
    token_endpoint: `${OIDC_AUTHORITY}/oauth/token`,
    userinfo_endpoint: `${OIDC_AUTHORITY}/userinfo`,
    jwks_uri: `${OIDC_AUTHORITY}/.well-known/jwks.json`,
    end_session_endpoint: `${OIDC_AUTHORITY}/v2/logout?client_id=${OIDC_CLIENT_ID}&returnTo=${encodeURIComponent(appBaseUrl)}`,
  },
};
