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
 * Respects the VITE_STRIP_ORG_ID env var for local development.
 */
export function getRuntimeBaseUrl(): string {
  const base = import.meta.env.VITE_RUNTARA_API_BASE_URL as string;
  if (!base) throw new Error('VITE_RUNTARA_API_BASE_URL is not configured');
  const stripOrgId = import.meta.env.VITE_STRIP_ORG_ID === 'true';
  const orgId = useAuthStore.getState().orgId;
  const orgSegment = orgId && !stripOrgId ? `/${orgId}` : '';
  return `${base.replace(/\/$/, '')}/api/runtime${orgSegment}`;
}
