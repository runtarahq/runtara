import { useEffect } from 'react';
import { useAuth } from 'react-oidc-context';
import { mountReturnPath, tenantIdFromUrl } from '@/shared/config/oidcConfig';
import { isOidcAuth } from '@/shared/config/runtimeConfig';

/**
 * If the SPA was loaded at `/ui/<tenant>/` but the JWT carries a different
 * `org_id`, redirect to `/ui/<jwt_org>/`. Without this, the SPA would render
 * but every API call returns 403 (server enforces JWT.org_id == TENANT_ID),
 * leaving the user staring at a broken UI.
 *
 * Also covers the "logged-in user lands at parent mount" case for symmetry
 * with `onSigninCallback` — anyone authenticated at `/ui/` gets sent to
 * their per-tenant SPA.
 *
 * No-op outside OIDC mode (single-tenant deploys have no notion of mismatch).
 */
export function useTenantUrlGuard() {
  const auth = useAuth();

  useEffect(() => {
    if (!isOidcAuth || !auth.isAuthenticated) return;

    const jwtOrgId = auth.user?.profile?.org_id as string | undefined;
    if (!jwtOrgId) return;

    if (tenantIdFromUrl === jwtOrgId) return;

    window.location.replace(`${mountReturnPath}${jwtOrgId}/`);
  }, [auth.isAuthenticated, auth.user?.profile]);
}
