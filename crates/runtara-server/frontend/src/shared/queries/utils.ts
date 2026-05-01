import { config } from '@/shared/config/runtimeConfig';
import { useAuthStore } from '@/shared/stores/authStore.ts';

/**
 * Creates authorization headers for API requests
 * @param token - The bearer token for authentication
 * @returns Headers object with Authorization header
 */
export const createAuthHeaders = (token: string) => ({
  headers: {
    Authorization: `Bearer ${token}`,
  },
});

/**
 * Returns the Runtime API base URL with org_id prefix.
 * For use in raw fetch() calls (Axios calls are handled by the interceptor).
 * Strip is controlled by config.stripOrgId (build-time VITE_STRIP_ORG_ID or
 * server-injected RUNTARA_UI_STRIP_ORG_ID).
 */
export function getRuntimeBaseUrl(): string {
  const base = config.apiBaseUrl.replace(/\/$/, '');
  const orgId = useAuthStore.getState().orgId;
  const orgSegment = orgId && !config.stripOrgId ? `/${orgId}` : '';
  return `${base}/api/runtime${orgSegment}`;
}
