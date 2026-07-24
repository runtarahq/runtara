import { Lock } from 'lucide-react';
import { Link } from 'react-router';
import { Button } from '@/shared/components/ui/button';
import { FEATURE_LABELS, type FeatureKey } from '@/shared/entitlements';

type FeatureDisabledProps = {
  feature: FeatureKey;
};

/**
 * Shown when a tenant navigates to a route whose feature is disabled in the
 * resolved entitlement snapshot. Mounted by `<EntitlementRoute>` — see
 * `docs/entitlements.md`.
 *
 * Intentionally minimal: no upgrade CTA (single-tenant deployments don't have
 * a billing flow yet), no branching by tier, no support link. The point is
 * to tell the user *why* the page they expected isn't there and where to go
 * instead.
 */
export function FeatureDisabled({ feature }: FeatureDisabledProps) {
  const label = FEATURE_LABELS[feature];

  return (
    <section
      role="region"
      aria-labelledby="feature-disabled-heading"
      className="flex min-h-[60vh] flex-col items-center justify-center px-6 text-center"
    >
      <Lock className="mb-4 size-12 text-muted-foreground" aria-hidden="true" />
      <h2 id="feature-disabled-heading" className="mb-2 text-2xl font-semibold">
        Feature not enabled
      </h2>
      <p className="mb-6 max-w-md text-muted-foreground">
        The <strong>{label}</strong> feature isn&apos;t included in your current
        plan. Contact your administrator if you believe this is unexpected.
      </p>
      <Button asChild variant="outline">
        <Link to="/workflows">Back to workflows</Link>
      </Button>
    </section>
  );
}
