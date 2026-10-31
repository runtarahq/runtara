import { useEffect } from 'react';
import { checkServerVersion } from '@/shared/config/serverCompat';
import { ManagementAPIClient } from '@/shared/queries';
import { useMaintenanceStore } from '@/shared/stores/maintenanceStore';

export function useHealthCheck() {
  const { isMaintenanceMode, setMaintenanceMode } = useMaintenanceStore();

  useEffect(() => {
    const checkHealth = async () => {
      try {
        const response = await ManagementAPIClient.instance.get('/health');
        setMaintenanceMode(false);
        checkServerVersion(response?.data?.version);
      } catch (error: any) {
        if (error?.response?.status === 503) {
          setMaintenanceMode(true);
        }
      }
    };

    checkHealth();
  }, [setMaintenanceMode]);

  // Poll while in maintenance mode to detect recovery
  useEffect(() => {
    if (!isMaintenanceMode) return;

    const interval = setInterval(async () => {
      try {
        await ManagementAPIClient.instance.get('/health');
        setMaintenanceMode(false);
      } catch {
        // still in maintenance
      }
    }, 30_000);

    return () => clearInterval(interval);
  }, [isMaintenanceMode, setMaintenanceMode]);

  return isMaintenanceMode;
}
