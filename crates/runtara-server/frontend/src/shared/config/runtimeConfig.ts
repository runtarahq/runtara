// Runtime configuration for the embedded UI.
//
// Values are injected by the Rust server into index.html as
// `window.__RUNTARA_CONFIG__` at startup, so the same compiled bundle can be
// deployed across tenants with different OIDC/API/analytics settings without
// rebuilding. When a key is missing (e.g. running via `vite dev` or in tests),
// we fall back to the build-time `VITE_*` values.

export type AuthMode = 'oidc' | 'local' | 'trust_proxy';

type RuntimeConfig = {
  oidcAuthority?: string;
  oidcClientId?: string;
  oidcAudience?: string;
  apiBaseUrl?: string;
  plausibleDomain?: string;
  plausibleHost?: string;
  version?: string;
  commit?: string;
  /** Selected server-side auth provider. Defaults to "oidc" when unset. */
  authMode?: AuthMode;
  /** Configured tenant — injected when the server runs without an IdP that
   * would otherwise supply `org_id` via JWT claims. */
  tenantId?: string;
  /** When "true", stop prefixing /api/runtime/ paths with org_id. Set by the
   * server from RUNTARA_UI_STRIP_ORG_ID for single-tenant deployments. */
  stripOrgId?: string;
};

declare global {
  interface Window {
    __RUNTARA_CONFIG__?: RuntimeConfig;
  }
}

const runtime: RuntimeConfig = window.__RUNTARA_CONFIG__ ?? {};

const clean = (value: string | undefined): string | undefined => {
  const trimmed = value?.trim();
  return trimmed ? trimmed : undefined;
};

const pick = (
  runtimeVal: string | undefined,
  envVal: string | undefined
): string | undefined => clean(runtimeVal) ?? clean(envVal);

const resolveAuthMode = (value: string | undefined): AuthMode => {
  switch (value) {
    case 'local':
      return 'local';
    case 'trust_proxy':
    case 'trust-proxy':
      return 'trust_proxy';
    default:
      return 'oidc';
  }
};

export const config = {
  authMode: resolveAuthMode(
    pick(runtime.authMode, import.meta.env.VITE_RUNTARA_AUTH_MODE)
  ),
  tenantId: pick(runtime.tenantId, import.meta.env.VITE_RUNTARA_TENANT_ID),
  stripOrgId:
    clean(runtime.stripOrgId) === 'true' ||
    import.meta.env.VITE_STRIP_ORG_ID === 'true',
  oidc: {
    authority: pick(runtime.oidcAuthority, import.meta.env.VITE_OIDC_AUTHORITY),
    clientId: pick(runtime.oidcClientId, import.meta.env.VITE_OIDC_CLIENT_ID),
    audience: pick(runtime.oidcAudience, import.meta.env.VITE_OIDC_AUDIENCE),
  },
  apiBaseUrl:
    pick(runtime.apiBaseUrl, import.meta.env.VITE_RUNTARA_API_BASE_URL) ?? '',
  plausible: {
    domain: pick(
      runtime.plausibleDomain,
      import.meta.env.VITE_RUNTARA_PLAUSIBLE_DOMAIN
    ),
    host: pick(
      runtime.plausibleHost,
      import.meta.env.VITE_RUNTARA_PLAUSIBLE_HOST
    ),
  },
  build: {
    version:
      pick(runtime.version, import.meta.env.VITE_RUNTARA_VERSION) ?? 'dev',
    commit: pick(runtime.commit, import.meta.env.VITE_RUNTARA_COMMIT),
  },
};

/** True when the server expects the SPA to perform OIDC auth itself. */
export const isOidcAuth = config.authMode === 'oidc';

// Diagnostic: make the resolved auth mode visible during boot so mismatches
// between server-injected config and what the SPA observes are easy to spot.
// Safe to leave in — logs at most once per page load.
console.info(
  `[runtara] authMode=${config.authMode} tenantId=${config.tenantId ?? '<unset>'} version=${config.build.version} commit=${config.build.commit ?? '<unset>'}`,
  window.__RUNTARA_CONFIG__
);
