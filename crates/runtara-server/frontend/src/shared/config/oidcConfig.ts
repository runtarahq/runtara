import { WebStorageStateStore } from 'oidc-client-ts';

const onSigninCallback = () => {
  window.history.replaceState({}, document.title, window.location.pathname);
};

const OIDC_AUTHORITY = import.meta.env.VITE_OIDC_AUTHORITY;
const OIDC_CLIENT_ID = import.meta.env.VITE_OIDC_CLIENT_ID;
const OIDC_AUDIENCE = import.meta.env.VITE_OIDC_AUDIENCE;

if (!OIDC_AUTHORITY || !OIDC_CLIENT_ID || !OIDC_AUDIENCE) {
  throw new Error(
    'Missing required OIDC environment variables: VITE_OIDC_AUTHORITY, VITE_OIDC_CLIENT_ID, VITE_OIDC_AUDIENCE'
  );
}

export const oidcConfig = {
  authority: OIDC_AUTHORITY,
  client_id: OIDC_CLIENT_ID,
  response_type: 'code',
  scope: 'email openid phone org_id',
  redirect_uri: window.location.origin,
  post_logout_redirect_uri: window.location.origin,
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
    end_session_endpoint: `${OIDC_AUTHORITY}/v2/logout?client_id=${OIDC_CLIENT_ID}&returnTo=${encodeURIComponent(window.location.origin)}`,
  },
};
