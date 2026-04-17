import { useEffect } from 'react';
import { useAuth } from 'react-oidc-context';
import { config, isOidcAuth } from '@/shared/config/runtimeConfig';
import { useAuthStore } from '@/shared/stores/authStore.ts';

export function useOrgId() {
  const auth = useAuth();
  const setOrgId = useAuthStore((state) => state.setOrgId);
  const clearOrgId = useAuthStore((state) => state.clearOrgId);

  useEffect(() => {
    // Non-OIDC modes: the server already knows its tenant. Take it from the
    // injected runtime config so the request interceptor can prefix URLs.
    if (!isOidcAuth) {
      setOrgId(config.tenantId ?? '');
      return;
    }

    if (auth.isAuthenticated) {
      const orgId = (auth.user?.profile?.org_id as string) ?? '';
      setOrgId(orgId);
    } else {
      clearOrgId();
    }
  }, [auth.isAuthenticated, auth.user?.profile, setOrgId, clearOrgId]);
}
