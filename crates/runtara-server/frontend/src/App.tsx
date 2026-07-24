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

function App() {
  const auth = useAutoSignin();

  useOrgId();
  useTenantUrlGuard();
  useUserGroups();
  useMe();
  const isMaintenanceMode = useHealthCheck();

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

  return <RouterProvider router={router} />;
}

export default App;
