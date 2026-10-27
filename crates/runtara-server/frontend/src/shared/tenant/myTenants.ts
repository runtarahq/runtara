import { useEffect, useState } from 'react';
import { useAuth } from 'react-oidc-context';
import { isOidcAuth } from '@/shared/config/runtimeConfig';
import { RuntimeREST } from '@/shared/queries';
import { createAuthHeaders } from '@/shared/queries/utils';

/** A tenant the signed-in user may act in, from `GET /api/runtime/me/tenants`. */
export type MyTenant = { id: string; role: string | null };

/** Where a user signed in without a tenant goes next. */
export type TenantChoice =
  | { kind: 'redirect'; id: string }
  | { kind: 'pick'; tenants: MyTenant[] }
  | { kind: 'none' };

/**
 * One tenant: go straight there. Several: let the user pick. None: there is
 * nowhere this user may act on this server.
 */
export function chooseTenant(tenants: MyTenant[]): TenantChoice {
  if (tenants.length === 1) return { kind: 'redirect', id: tenants[0].id };
  if (tenants.length > 1) return { kind: 'pick', tenants };
  return { kind: 'none' };
}

/** The SPA root of `tenant` under the mount (e.g. `/ui/` → `/ui/acme/`). */
export function tenantHome(mountPath: string, tenant: string): string {
  const mount = mountPath.endsWith('/') ? mountPath : `${mountPath}/`;
  return `${mount}${encodeURIComponent(tenant)}/`;
}

/** Whether the user may open `tenant`; `null` until that is known. */
export function canOpen(
  tenants: MyTenant[] | null,
  tenant: string
): boolean | null {
  return tenants === null ? null : tenants.some((t) => t.id === tenant);
}

/**
 * The tenants the signed-in user may act in. `null` while loading, outside
 * OIDC (where the server's own tenant is the only one), and when the lookup
 * failed, so a failure is never read as "a member of nothing".
 */
export function useMyTenants(): MyTenant[] | null {
  const auth = useAuth();
  const token = auth.user?.access_token;
  const [tenants, setTenants] = useState<MyTenant[] | null>(null);

  useEffect(() => {
    if (!isOidcAuth || !auth.isAuthenticated) {
      setTenants(null);
      return;
    }
    let cancelled = false;
    RuntimeREST.instance
      .get('/api/runtime/me/tenants', createAuthHeaders(token))
      .then((res) => {
        if (!cancelled) setTenants(res.data?.tenants ?? []);
      })
      .catch(() => {
        if (!cancelled) setTenants(null);
      });
    return () => {
      cancelled = true;
    };
  }, [auth.isAuthenticated, token]);

  return tenants;
}
