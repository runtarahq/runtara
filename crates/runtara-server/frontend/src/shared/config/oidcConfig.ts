import type { User } from 'oidc-client-ts';
import { WebStorageStateStore } from 'oidc-client-ts';
import { config, isOidcAuth } from '@/shared/config/runtimeConfig';

// Full absolute URL of the SPA root (protocol + host + mount path), e.g.
// `http://localhost:7001/ui/org_abc/`. `document.baseURI` reflects the
// `<base href>` the server injects at startup, so per-tenant deploys see
// the tenant-scoped path here.
const appBaseUrl = document.baseURI;

// Auth0's "Allowed Callback URLs" / "Allowed Logout URLs" lists are per
// *application*, not per *tenant* — so a per-tenant `<base href>` (e.g.
// `/ui/org_abc/`) can never be whitelisted directly. Strip down to the
// parent mount (`/ui/`); one whitelist entry then covers every tenant.
// After Auth0 callback lands at `/ui/`, `onSigninCallback` redirects to
// the per-tenant mount based on the JWT's `org_id` claim. localStorage is
// keyed by origin (not path), so the per-tenant SPA inherits the OIDC state.
const baseUrlParts = new URL(appBaseUrl);
const baseUrlSegments = baseUrlParts.pathname.split('/').filter(Boolean);
const mountSegment = baseUrlSegments[0] ?? 'ui';
const mountReturnUrl = `${baseUrlParts.origin}/${mountSegment}/`;

/** The path Auth0 callbacks always return to (e.g. `/ui/`). */
export const mountReturnPath = new URL(mountReturnUrl).pathname;

/**
 * Tenant id parsed from the SPA's mount path, e.g. `/ui/org_abc/` → `org_abc`.
 * `undefined` when loaded at the parent mount (`/ui/`) or bare origin — i.e.
 * during the OIDC callback hop, before we know which tenant to land in.
 */
export const tenantIdFromUrl: string | undefined = baseUrlSegments[1];

const onSigninCallback = (user?: User) => {
  const orgId = user?.profile?.org_id as string | undefined;

  // Auth0 always returns to the whitelistable parent mount (`/ui/`). When the
  // JWT carries an `org_id`, swap to the per-tenant SPA — same origin, so
  // localStorage carries the OIDC state and the new SPA skips re-auth.
  if (orgId && window.location.pathname === mountReturnPath) {
    window.location.replace(`${mountReturnPath}${orgId}/`);
    return;
  }

  // Already on a per-tenant path (or no org_id): just clean the auth params
  // off the URL.
  window.history.replaceState({}, document.title, window.location.pathname);
};

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
      end_session_endpoint: `${authority}/v2/logout?client_id=${clientId}&returnTo=${encodeURIComponent(mountReturnUrl)}`,
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
  redirect_uri: mountReturnUrl,
  post_logout_redirect_uri: mountReturnUrl,
  extraTokenParams: OIDC_AUDIENCE ? { audience: OIDC_AUDIENCE } : undefined,
  // Pass the tenant id from the URL as Auth0's `organization` param so a
  // multi-org user lands directly in the org they tried to deep-link to,
  // skipping the org-selector. If they aren't a member, Auth0 errors cleanly
  // instead of silently authenticating them into their default org.
  extraQueryParams: {
    ...(OIDC_AUDIENCE ? { audience: OIDC_AUDIENCE } : {}),
    ...(tenantIdFromUrl ? { organization: tenantIdFromUrl } : {}),
  },
  automaticSilentRenew: isOidcAuth,
  userStore: new WebStorageStateStore({ store: window.localStorage }),
  onSigninCallback,
  metadata,
};
