// Runtime configuration for the embedded UI.
//
// Values are injected by the Rust server into index.html as
// `window.__RUNTARA_CONFIG__` at startup, so the same compiled bundle can be
// deployed across tenants with different OIDC/API/analytics settings without
// rebuilding. When a key is missing (e.g. running via `vite dev` or in tests),
// we fall back to the build-time `VITE_*` values.

type RuntimeConfig = {
  oidcAuthority?: string;
  oidcClientId?: string;
  oidcAudience?: string;
  apiBaseUrl?: string;
  plausibleDomain?: string;
  plausibleHost?: string;
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

export const config = {
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
};
