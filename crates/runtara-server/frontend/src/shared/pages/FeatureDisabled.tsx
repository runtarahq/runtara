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
 * Phase 4.4 in `docs/entitlements.md`.
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
      className="flex flex-col items-center justify-center min-h-[60vh] px-6 text-center"
    >
      <Lock
        className="size-12 text-muted-foreground mb-4"
        aria-hidden="true"
      />
      <h2
        id="feature-disabled-heading"
        className="text-2xl font-semibold mb-2"
      >
        Feature not enabled
      </h2>
      <p className="max-w-md text-muted-foreground mb-6">
        The <strong>{label}</strong> feature isn&apos;t included in your current
        plan. Contact your administrator if you believe this is unexpected.
      </p>
      <Button asChild variant="outline">
        <Link to="/workflows">Back to workflows</Link>
      </Button>
    </section>
  );
}
