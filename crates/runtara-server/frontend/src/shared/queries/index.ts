import * as ManagementAPI from '@/generated/RuntaraManagementApi.ts';
import * as RuntimeAPI from '@/generated/RuntaraRuntimeApi.ts';
import { useAuthStore } from '@/shared/stores/authStore.ts';
import { useMaintenanceStore } from '@/shared/stores/maintenanceStore.ts';

// Validate required environment variables at startup
const API_BASE_URL = import.meta.env.VITE_RUNTARA_API_BASE_URL as
  | string
  | undefined;
if (!API_BASE_URL) {
  throw new Error(
    'Missing required environment variable: VITE_RUNTARA_API_BASE_URL. ' +
      'Check your .env file or deployment configuration.'
  );
}

/**
 * Management API client - single shared instance
 * Used for management operations
 */
export const ManagementAPIClient = new ManagementAPI.Api({
  format: 'json',
  baseURL: API_BASE_URL,
});

/**
 * Runtime API client - single shared instance
 * All features should import this instead of creating their own instance
 */
export const RuntimeREST = new RuntimeAPI.Api({
  format: 'json',
  baseURL: API_BASE_URL,
});

// Interceptor: insert org_id into /api/runtime/ paths (skipped when VITE_STRIP_ORG_ID is set)
const stripOrgId = import.meta.env.VITE_STRIP_ORG_ID === 'true';
RuntimeREST.instance.interceptors.request.use((config) => {
  const orgId = useAuthStore.getState().orgId;
  if (orgId && config.url && !stripOrgId) {
    config.url = config.url.replace(
      /\/api\/runtime\//,
      `/api/runtime/${orgId}/`
    );
  }
  return config;
});

// Interceptor: detect 503 (Service Unavailable) and activate maintenance mode
const maintenanceInterceptor = (error: any) => {
  if (error?.response?.status === 503) {
    useMaintenanceStore.getState().setMaintenanceMode(true);
  }
  return Promise.reject(error);
};

// Interceptor: redirect to login on 401 Unauthorized (expired/invalid token)
let isRedirectingToLogin = false;
const unauthorizedInterceptor = (error: any) => {
  if (error?.response?.status === 401 && !isRedirectingToLogin) {
    isRedirectingToLogin = true;
    useAuthStore.getState().clearOrgId();
    useAuthStore.getState().clearUserGroups();
    // Clear OIDC session from storage before redirecting to prevent
    // re-authentication loop (useAutoSignin would re-auth, hit 401 again)
    for (const key of Object.keys(localStorage)) {
      if (key.startsWith('oidc.')) {
        localStorage.removeItem(key);
      }
    }
    window.location.href = window.location.origin;
  }
  return Promise.reject(error);
};

ManagementAPIClient.instance.interceptors.response.use(
  undefined,
  maintenanceInterceptor
);
ManagementAPIClient.instance.interceptors.response.use(
  undefined,
  unauthorizedInterceptor
);
RuntimeREST.instance.interceptors.response.use(
  undefined,
  maintenanceInterceptor
);
RuntimeREST.instance.interceptors.response.use(
  undefined,
  unauthorizedInterceptor
);

// TODO: OAuth authorization redirect endpoint not yet available in RuntimeAPI
// This function will need to be updated once the endpoint is added to RuntimeAPI
/** @lintignore Public stub retained until OAuth redirect endpoint lands in RuntimeAPI. */
export async function getAuthorizationRedirect(
  _token: string,
  context: any
): Promise<string> {
  const [, integrationId] = context.queryKey;
  console.warn(
    `OAuth authorization redirect for integration ${integrationId} is not yet available in RuntimeAPI`
  );
  throw new Error(
    'OAuth authorization redirect is not yet available. Please configure connections manually.'
  );
}

export * from './billing';
