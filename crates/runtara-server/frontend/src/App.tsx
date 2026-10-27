import { RouterProvider } from 'react-router/dom';
import { router } from '@/router';
import { Loader } from '@/shared/components/loader.tsx';
import { useAutoSignin } from '@/shared/hooks/useAutoSignin';
import { useMe } from '@/shared/hooks/useMe';
import { useOrgId } from '@/shared/hooks/useOrgId';
import { useTenantUrlGuard } from '@/shared/hooks/useTenantUrlGuard';
import { useUserGroups } from '@/shared/hooks/useUserGroups';
import { useEffect } from 'react';
import { cleanupPointerEvents } from '@/lib/utils';
import { MaintenancePage } from '@/shared/components/maintenance-page';
import { useHealthCheck } from '@/shared/hooks/useHealthCheck';
import { tenantIdFromUrl } from '@/shared/config/oidcConfig';
import { isOidcAuth } from '@/shared/config/runtimeConfig';
import { TenantGate } from '@/shared/tenant/TenantGate';
import { canOpen, useMyTenants } from '@/shared/tenant/myTenants';

function App() {
  const auth = useAutoSignin();

  useOrgId();
  useTenantUrlGuard();
  useUserGroups();
  useMe();
  const isMaintenanceMode = useHealthCheck();
  const myTenants = useMyTenants();

  // Global cleanup for pointer-events
  useEffect(() => {
    // Clean up on mount
    cleanupPointerEvents();

    // Clean up on unmount
    return () => {
      cleanupPointerEvents();
    };
  }, []);

  if (auth.isLoading) {
    return <Loader />;
  }

  if (auth.error) {
    return <div>Encountering error... {auth.error.message}</div>;
  }

  if (isMaintenanceMode) {
    return <MaintenancePage />;
  }

  // Signed in with a token that names no tenant: at the bare mount the user
  // picks (or is sent to) a workspace they belong to, and at a workspace they
  // do not belong to they are told so instead of shown a page of 403s.
  const tenantless =
    isOidcAuth && auth.isAuthenticated && !auth.user?.profile?.org_id;
  if (tenantless && !tenantIdFromUrl) {
    return <TenantGate tenants={myTenants} />;
  }
  if (
    tenantless &&
    tenantIdFromUrl &&
    canOpen(myTenants, tenantIdFromUrl) === false
  ) {
    return <TenantGate tenants={myTenants} deniedTenant={tenantIdFromUrl} />;
  }

  return <RouterProvider router={router} />;
}

export default App;
