import { WebStorageStateStore } from 'oidc-client-ts';
import { config, isOidcAuth } from '@/shared/config/runtimeConfig';

const onSigninCallback = () => {
  window.history.replaceState({}, document.title, window.location.pathname);
};

// Full absolute URL of the SPA root (protocol + host + mount path), e.g.
// `http://localhost:7001/ui/org_abc/`. `document.baseURI` reflects the
// `<base href>` the server injects at startup, so Auth0 redirects land
// back on the tenant-scoped mount instead of the bare origin.
const appBaseUrl = document.baseURI;

const OIDC_AUTHORITY = config.oidc.authority;
const OIDC_CLIENT_ID = config.oidc.clientId;
const OIDC_AUDIENCE = config.oidc.audience;

if (isOidcAuth && (!OIDC_AUTHORITY || !OIDC_CLIENT_ID || !OIDC_AUDIENCE)) {
  throw new Error(
    'Missing required OIDC configuration. Provide either window.__RUNTARA_CONFIG__.oidc{Authority,ClientId,Audience} (injected by the server from RUNTARA_UI_OIDC_* env vars) or VITE_OIDC_AUTHORITY / VITE_OIDC_CLIENT_ID / VITE_OIDC_AUDIENCE at build time.'
  );
}

// Non-OIDC auth modes (`local`, `trust_proxy`) still mount `<AuthProvider>` so
// call sites can keep using `useAuth()`, but `useAutoSignin` won't fire the
// redirect and `PrivateRoute` short-circuits. We supply syntactically valid
// stub endpoints so `oidc-client-ts` doesn't attempt discovery against a real
// IdP; nothing actually calls them because `signinRedirect` is never invoked.
const STUB_AUTHORITY = `${window.location.origin}/__runtara_no_auth__`;
const authority = OIDC_AUTHORITY ?? STUB_AUTHORITY;
const clientId = OIDC_CLIENT_ID ?? 'runtara-no-auth';

const metadata = isOidcAuth
  ? {
      issuer: `${authority}/`,
      authorization_endpoint: `${authority}/authorize`,
      token_endpoint: `${authority}/oauth/token`,
      userinfo_endpoint: `${authority}/userinfo`,
      jwks_uri: `${authority}/.well-known/jwks.json`,
      end_session_endpoint: `${authority}/v2/logout?client_id=${clientId}&returnTo=${encodeURIComponent(appBaseUrl)}`,
    }
  : {
      // Non-OIDC stub: endpoints point at the local SPA so the client library
      // is well-formed but any outbound call would 404 same-origin instead of
      // leaking to an external host. Never exercised in practice.
      issuer: `${authority}/`,
      authorization_endpoint: `${authority}/authorize`,
      token_endpoint: `${authority}/token`,
      userinfo_endpoint: `${authority}/userinfo`,
      jwks_uri: `${authority}/jwks.json`,
      end_session_endpoint: `${authority}/logout`,
    };

export const oidcConfig = {
  authority,
  client_id: clientId,
  response_type: 'code',
  scope: 'email openid phone org_id',
  redirect_uri: appBaseUrl,
  post_logout_redirect_uri: appBaseUrl,
  extraTokenParams: OIDC_AUDIENCE ? { audience: OIDC_AUDIENCE } : undefined,
  extraQueryParams: OIDC_AUDIENCE ? { audience: OIDC_AUDIENCE } : undefined,
  automaticSilentRenew: isOidcAuth,
  userStore: new WebStorageStateStore({ store: window.localStorage }),
  onSigninCallback,
  metadata,
};
