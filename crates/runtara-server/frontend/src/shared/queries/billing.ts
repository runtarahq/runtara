import { ManagementAPIClient } from './index';
import { createAuthHeaders } from './utils';

/**
 * Opens the managed deployment's billing portal. Nothing in this console calls
 * it: a distribution's edition (`src/shared/edition`) does.
 * @public
 */
export async function createBillingPortalSession(token: string) {
  const result = await ManagementAPIClient.api.createBillingPortalSession(
    {
      return_url: window.location.origin,
    },
    createAuthHeaders(token)
  );

  return result.data;
}
