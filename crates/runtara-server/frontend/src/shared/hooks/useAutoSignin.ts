import { useEffect, useState } from 'react';
import { hasAuthParams, useAuth } from 'react-oidc-context';

export function useAutoSignin() {
  const auth = useAuth();

  const [hasTriedSignin, setHasTriedSignin] = useState(false);

  useEffect(() => {
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
