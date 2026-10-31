// Static hosting: the bundle served by a plain static host (a CDN) instead of
// runtara-server. The server injects `window.__RUNTARA_CONFIG__` into
// index.html; a static host serves the same index.html to every path and
// tenant, so the deployment's settings come from a `config.json` next to it.
//
// Runs before anything reads the config (see main.tsx): runtimeConfig.ts
// snapshots `window.__RUNTARA_CONFIG__` when it is first imported.
//
// Imports nothing at runtime, so loading it evaluates no other module.

import type { RuntimeConfig } from '@/shared/config/runtimeConfig';

/**
 * `mountPath` with exactly one leading and one trailing slash; `/ui/`, the
 * server's mount, when unset.
 */
export function normalizeMountPath(mountPath: string | undefined): string {
  if (!mountPath?.trim()) return '/ui/';
  const segments = mountPath.split('/').filter(Boolean);
  return segments.length ? `/${segments.join('/')}/` : '/';
}

/** Whether `pathname` is the mount or below it. */
export function isWithinMount(pathname: string, mountPath: string): boolean {
  return pathname.startsWith(mountPath) || `${pathname}/` === mountPath;
}

/**
 * The deployment's settings from `config.json`, or `undefined` when there is
 * none to read. Keys only a build or a server can know are dropped: the
 * version stamp is the bundle's own (`VITE_RUNTARA_*`), and entitlements are
 * the tenant's, read from `GET /api/runtime/entitlements`.
 */
export function parseStaticConfig(loaded: unknown): RuntimeConfig | undefined {
  if (!loaded || typeof loaded !== 'object' || Array.isArray(loaded)) {
    return undefined;
  }
  const settings = { ...(loaded as Record<string, unknown>) };
  for (const key of ['version', 'commit', 'buildNumber', 'entitlements']) {
    delete settings[key];
  }
  return { ...(settings as RuntimeConfig), hosting: 'static' };
}

/**
 * Read `config.json` when the server injected no config. Returns `false` when
 * the page is leaving for the mount, so the caller boots nothing.
 *
 * Never throws: with no `config.json` (vite dev, a server that injected the
 * config) the build-time `VITE_*` values stay in force.
 */
export async function loadStaticConfig(): Promise<boolean> {
  // The server always injects `authMode`; index.html's placeholder is `{}`.
  if (window.__RUNTARA_CONFIG__?.authMode) return true;

  let config: RuntimeConfig | undefined;
  try {
    const response = await fetch(new URL('config.json', document.baseURI), {
      headers: { accept: 'application/json' },
      cache: 'no-store',
    });
    // A static host's SPA fallback (vite dev, Cloudflare Pages) answers a
    // missing config.json with index.html and a 200.
    const type = response.headers.get('content-type') ?? '';
    if (response.ok && type.includes('application/json')) {
      config = parseStaticConfig(await response.json());
    }
  } catch (error) {
    console.warn('[runtara] config.json could not be read', error);
  }
  if (!config) return true;

  window.__RUNTARA_CONFIG__ = config;

  // Everything the console shows is under the mount (its tenants at
  // `{mount}{tenant}/`), so a page outside it (the bare origin, where a 401
  // sends the user) goes to the mount.
  const mountPath = normalizeMountPath(config.mountPath);
  if (!isWithinMount(window.location.pathname, mountPath)) {
    window.location.replace(mountPath);
    return false;
  }
  return true;
}
