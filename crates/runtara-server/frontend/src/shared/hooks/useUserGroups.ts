import { useEffect } from 'react';
import { useShallow } from 'zustand/react/shallow';
import { useAuth } from 'react-oidc-context';
import { useAuthStore } from '@/shared/stores/authStore.ts';

export function useUserGroups() {
  const auth = useAuth();

  const { setUserGroups, clearUserGroups } = useAuthStore(
    useShallow((state) => ({
      setUserGroups: state.setUserGroups,
      clearUserGroups: state.clearUserGroups,
    }))
  );

  useEffect(() => {
    if (auth.isAuthenticated) {
      const groups = (auth.user?.profile['cognito:groups'] as string[]) ?? [];
      setUserGroups(groups);
    } else {
      clearUserGroups();
    }
  }, [
    auth.isAuthenticated,
    auth.user?.profile,
    setUserGroups,
    clearUserGroups,
  ]);
}
