import { useEffect, useState } from 'react';
import { hasAuthParams, useAuth } from 'react-oidc-context';
import { isOidcAuth } from '@/shared/config/runtimeConfig';

export function useAutoSignin() {
  const auth = useAuth();

  const [hasTriedSignin, setHasTriedSignin] = useState(false);

  useEffect(() => {
    // In local / trust_proxy modes the server accepts every request; never
    // redirect to an IdP we don't need.
    if (!isOidcAuth) {
      return;
    }

    if (
      !hasAuthParams() &&
      !auth.isAuthenticated &&
      !auth.activeNavigator &&
      !auth.isLoading &&
      !hasTriedSignin
    ) {
      auth.signinRedirect();
      setHasTriedSignin(true);
    }
  }, [
    auth.isAuthenticated,
    auth.activeNavigator,
    auth.isLoading,
    hasTriedSignin,
  ]);

  return auth;
}
