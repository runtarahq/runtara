import { matchPath } from 'react-router';
import { config } from '@/shared/config/runtimeConfig';
import { isWithinMount } from '@/shared/config/staticConfig';
import { edition } from '@edition';

/** Where the SPA is rooted: the mount, or one of its tenants under it. */
export type AppBase = { path: string; tenant?: string };

// A tenant id as the path may carry it. The console builds API paths from the
// tenant, so anything else (`..`, an escape) could aim a signed-in user's
// requests elsewhere: such a segment names no tenant.
const TENANT_SEGMENT = /^[A-Za-z0-9_-]{1,128}$/;

/**
 * The SPA's root under a static host, from the page's path. Below the mount,
 * the first segment names the tenant (`/ui/acme/workflows` → `/ui/acme/`),
 * unless the path is one of the edition's open pages, which are served at the
 * mount for any signed-in user.
 */
export function resolveStaticBase(
  pathname: string,
  mountPath: string,
  openPaths: string[]
): AppBase {
  if (!isWithinMount(pathname, mountPath)) return { path: mountPath };

  const rest = pathname.slice(mountPath.length);
  if (openPaths.some((path) => matchPath(path, `/${rest}`) !== null)) {
    return { path: mountPath };
  }

  const segment = rest.split('/')[0];
  if (!TENANT_SEGMENT.test(segment)) return { path: mountPath };
  return { path: `${mountPath}${segment}/`, tenant: segment };
}

const staticBase =
  config.hosting === 'static'
    ? resolveStaticBase(
        window.location.pathname,
        config.mountPath,
        edition.open.map(({ path }) => path)
      )
    : undefined;

/**
 * Full absolute URL of the SPA root (protocol + host + mount path), e.g.
 * `http://localhost:7001/ui/org_abc/`. Served by runtara-server, that is
 * `document.baseURI`: the `<base href>` it injects for the tenant. A static
 * host serves one index.html everywhere, so there it comes from the page's
 * path.
 */
export const appBaseUrl = staticBase
  ? new URL(staticBase.path, window.location.origin).href
  : document.baseURI;

const baseSegments = new URL(appBaseUrl).pathname.split('/').filter(Boolean);

/**
 * The parent mount every tenant is under (e.g. `/ui/`): where OIDC callbacks
 * return, since an IdP's allowed callback URLs can't list every tenant.
 */
export const mountReturnPath = staticBase
  ? config.mountPath
  : `/${baseSegments[0] ?? 'ui'}/`;

/**
 * Tenant id parsed from the SPA's mount path, e.g. `/ui/org_abc/` → `org_abc`.
 * `undefined` when loaded at the parent mount (`/ui/`) or bare origin — i.e.
 * during the OIDC callback hop, before we know which tenant to land in.
 */
export const tenantIdFromUrl: string | undefined = staticBase
  ? staticBase.tenant
  : baseSegments[1];
