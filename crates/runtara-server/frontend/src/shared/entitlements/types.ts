// Domain re-exports from the generated API client.
//
// The generator names everything with a `Dto` suffix; the rest of the SPA
// should refer to entitlement concepts by their domain names. Keeping one
// thin module here means call sites don't import from `generated/` directly,
// and a future generator change can be absorbed in one file.

export type {
  EntitlementsDto as EntitlementsSnapshot,
  EntitlementLimits,
  FeatureKey,
  Tier as PricingTier,
} from '@/generated/RuntaraRuntimeApi';
