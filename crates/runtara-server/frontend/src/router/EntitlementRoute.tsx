import React from 'react';
import { useEntitlements } from '@/shared/hooks/useEntitlements';
import { isEnabled, type FeatureKey } from '@/shared/entitlements';
import { FeatureDisabled } from '@/shared/pages/FeatureDisabled';

interface EntitlementRouteProps {
  feature: FeatureKey;
  children: React.ReactNode;
}

/**
 * Route guard for entitlement-gated pages. Renders `<FeatureDisabled>` when
 * the feature is off in the resolved snapshot, otherwise the children.
 *
 * Composes inside `<PrivateRoute>`: auth-then-entitlement. An unauthenticated
 * user hitting a gated URL must still go through login first; only then do
 * we decide whether they're allowed to *see* the feature. See
 * `docs/entitlements.md`.
 *
 * Hidden menu items in the sidebar are not enough on their own — a user can
 * paste the URL directly. This wrapper is the second-line defense.
 */
export function EntitlementRoute({ feature, children }: EntitlementRouteProps) {
  const entitlements = useEntitlements();
  if (!isEnabled(entitlements, feature)) {
    return <FeatureDisabled feature={feature} />;
  }
  return <>{children}</>;
}
