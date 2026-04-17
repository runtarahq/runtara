import { RouterProvider } from 'react-router/dom';
import { router } from '@/router';
import { Loader } from '@/shared/components/loader.tsx';
import { useAutoSignin } from '@/shared/hooks/useAutoSignin';
import { useOrgId } from '@/shared/hooks/useOrgId';
import { useUserGroups } from '@/shared/hooks/useUserGroups';
import { useEffect } from 'react';
import { cleanupPointerEvents } from '@/lib/utils';
import { useThemeStore } from '@/shared/stores/themeStore';
import { OfflineIndicator, PWAUpdatePrompt } from '@/shared/components/pwa';
import { MaintenancePage } from '@/shared/components/maintenance-page';
import { useHealthCheck } from '@/shared/hooks/useHealthCheck';

function App() {
  const auth = useAutoSignin();
  const { theme } = useThemeStore();

  useOrgId();
  useUserGroups();
  const isMaintenanceMode = useHealthCheck();

  // Initialize theme
  useEffect(() => {
    const root = document.documentElement;
    const isDark =
      theme === 'dark' ||
      (theme === 'system' &&
        window.matchMedia('(prefers-color-scheme: dark)').matches);

    if (isDark) {
      root.classList.add('dark');
    } else {
      root.classList.remove('dark');
    }
  }, [theme]);

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

  return (
    <>
      <OfflineIndicator />
      <RouterProvider router={router} />
      <PWAUpdatePrompt />
    </>
  );
}

export default App;
