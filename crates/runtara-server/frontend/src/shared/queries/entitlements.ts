import { RuntimeREST } from './index';
import { createAuthHeaders } from './utils';
import type { EntitlementsSnapshot } from '@/shared/entitlements';

/**
 * Fetch the resolved entitlement snapshot for the authenticated tenant.
 *
 * Returns the same shape inlined into `window.__RUNTARA_CONFIG__.entitlements`
 * — see `crates/runtara-server/src/api/handlers/entitlements.rs`.
 */
export async function fetchEntitlements(
  token?: string | null
): Promise<EntitlementsSnapshot> {
  const result = await RuntimeREST.api.getEntitlementsHandler(
    createAuthHeaders(token)
  );
  return result.data;
}
