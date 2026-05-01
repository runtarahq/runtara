import * as ManagementAPI from '@/generated/RuntaraManagementApi.ts';
import * as RuntimeAPI from '@/generated/RuntaraRuntimeApi.ts';
import { config, isOidcAuth } from '@/shared/config/runtimeConfig';
import { useAuthStore } from '@/shared/stores/authStore.ts';
import { useMaintenanceStore } from '@/shared/stores/maintenanceStore.ts';

// Resolve the API base URL. When unset (typical for the embedded UI where the
// API is same-origin), we use relative URLs and let the browser resolve them.
const API_BASE_URL = config.apiBaseUrl;

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

// Interceptor: insert org_id into /api/runtime/ paths (skipped when stripOrgId
// is set — by build-time VITE_STRIP_ORG_ID or runtime RUNTARA_UI_STRIP_ORG_ID).
RuntimeREST.instance.interceptors.request.use((requestConfig) => {
  const orgId = useAuthStore.getState().orgId;
  if (orgId && requestConfig.url && !config.stripOrgId) {
    requestConfig.url = requestConfig.url.replace(
      /\/api\/runtime\//,
      `/api/runtime/${orgId}/`
    );
  }
  return requestConfig;
});

// Interceptor: detect 503 (Service Unavailable) and activate maintenance mode
const maintenanceInterceptor = (error: any) => {
  if (error?.response?.status === 503) {
    useMaintenanceStore.getState().setMaintenanceMode(true);
  }
  return Promise.reject(error);
};

// Interceptor: redirect to login on 401 Unauthorized (expired/invalid token).
// Non-OIDC modes have no login to return to, so a 401 there is a genuine server
// error — propagate it to the caller instead of triggering a redirect loop.
let isRedirectingToLogin = false;
const unauthorizedInterceptor = (error: any) => {
  if (
    isOidcAuth &&
    error?.response?.status === 401 &&
    !isRedirectingToLogin
  ) {
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
