import { useCustomQuery } from './api';
import {
  PERMISSIVE_FALLBACK,
  type EntitlementsSnapshot,
} from '@/shared/entitlements';
import { fetchEntitlements } from '@/shared/queries/entitlements';

const ENTITLEMENTS_QUERY_KEY = ['entitlements'] as const;

/**
 * Resolve the per-process entitlement snapshot.
 *
 * Resolution order:
 *   1. `window.__RUNTARA_CONFIG__.entitlements` — inlined by the Rust UI
 *      handler at HTML serve time (always present in production).
 *   2. `GET /api/runtime/entitlements` — used when the SPA is served outside
 *      the Rust binary (`vite dev`, tests, Storybook).
 *   3. `PERMISSIVE_FALLBACK` — last-resort default matching the backend's
 *      "no entitlement env set" behavior, so a misconfigured server can't
 *      black-screen the UI.
 *
 * The snapshot is process-stable on the backend (an `OnceLock` populated at
 * startup), so the query uses `staleTime: Infinity` and never refetches on
 * focus or mount.
 */
export function useEntitlements(): EntitlementsSnapshot {
  const inlined = window.__RUNTARA_CONFIG__?.entitlements;

  const query = useCustomQuery<EntitlementsSnapshot>({
    queryKey: ENTITLEMENTS_QUERY_KEY,
    queryFn: (token) => fetchEntitlements(token),
    enabled: !inlined,
    staleTime: Infinity,
    gcTime: Infinity,
    retry: false,
  });

  return inlined ?? query.data ?? PERMISSIVE_FALLBACK;
}
