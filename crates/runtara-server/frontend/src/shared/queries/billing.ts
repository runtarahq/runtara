import { ManagementAPIClient } from './index';
import { createAuthHeaders } from './utils';

export async function createBillingPortalSession(token: string) {
  const result = await ManagementAPIClient.api.createBillingPortalSession(
    {
      return_url: window.location.origin,
    },
    createAuthHeaders(token)
  );

  return result.data;
}
