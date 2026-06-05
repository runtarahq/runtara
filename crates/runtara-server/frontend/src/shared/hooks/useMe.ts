import { useEffect } from 'react';
import { useShallow } from 'zustand/react/shallow';
import { useAuth } from 'react-oidc-context';
import { isOidcAuth } from '@/shared/config/runtimeConfig';
import { useAuthStore } from '@/shared/stores/authStore.ts';
import { RuntimeREST } from '@/shared/queries';
import { createAuthHeaders } from '@/shared/queries/utils';

/**
 * Fetch the authoritative identity view from `GET /api/runtime/me` and store the
 * caller's role + effective permissions. Unlike token-derived `userGroups`, the
 * role here is read server-side from the per-tenant Valkey membership entry, so a
 * role change or removal is reflected on the next load — never cached in the JWT.
 *
 * Mirrors `useOrgId`/`useUserGroups`: runs after auth bootstrap, clears on sign-out.
 */
export function useMe() {
  const auth = useAuth();
  const { setMe, clearMe } = useAuthStore(
    useShallow((state) => ({ setMe: state.setMe, clearMe: state.clearMe }))
  );

  // OIDC modes gate on an authenticated session; non-OIDC modes (local /
  // trust_proxy) are always "ready" — the server resolves the tenant itself.
  const ready = isOidcAuth ? auth.isAuthenticated : true;
  const token = auth.user?.access_token;

  useEffect(() => {
    if (!ready) {
      clearMe();
      return;
    }

    let cancelled = false;
    RuntimeREST.instance
      .get('/api/runtime/me', isOidcAuth ? createAuthHeaders(token) : undefined)
      .then((res) => {
        if (cancelled) return;
        setMe({
          role: res.data?.role ?? null,
          permissions: res.data?.permissions ?? {},
        });
      })
      .catch(() => {
        if (!cancelled) clearMe();
      });

    return () => {
      cancelled = true;
    };
  }, [ready, token, setMe, clearMe]);
}
