import React from 'react';
import { useAuth } from 'react-oidc-context';
import { Navigate } from 'react-router';
import { isOidcAuth } from '@/shared/config/runtimeConfig';

interface PrivateRouteProps {
  children: React.ReactNode;
}

const PageLoader = () => (
  <div className="flex items-center justify-center h-full">
    <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-primary"></div>
  </div>
);

export function PrivateRoute(props: PrivateRouteProps) {
  const { children } = props;

  const auth = useAuth();

  // Server handles (or skips) auth upstream in non-OIDC modes, so there's no
  // login gate to enforce here.
  if (!isOidcAuth) {
    return children;
  }

  if (auth.isLoading) {
    return <PageLoader />;
  }

  return auth.isAuthenticated ? children : <Navigate to="/login" />;
}
