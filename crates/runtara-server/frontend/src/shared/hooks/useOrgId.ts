import { useEffect } from 'react';
import { useAuth } from 'react-oidc-context';
import { useAuthStore } from '@/shared/stores/authStore.ts';

export function useOrgId() {
  const auth = useAuth();
  const setOrgId = useAuthStore((state) => state.setOrgId);
  const clearOrgId = useAuthStore((state) => state.clearOrgId);

  useEffect(() => {
    if (auth.isAuthenticated) {
      const orgId = (auth.user?.profile?.org_id as string) ?? '';
      setOrgId(orgId);
    } else {
      clearOrgId();
    }
  }, [auth.isAuthenticated, auth.user?.profile, setOrgId, clearOrgId]);
}
