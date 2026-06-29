import type { RateLimitStatusDto } from '@/generated/RuntaraRuntimeApi';

/**
 * Honest rate-limit "protection state" for a connection.
 *
 * Single source of truth shared by the analytics dashboard card and the
 * connection detail view so they can never disagree. The key correctness
 * property (SYN-495): a connection with **no config** is shown as neutral
 * "No limit" — never the green "OK" success badge that implies protection.
 */
export type RateLimitBadgeVariant =
  | 'muted'
  | 'warning'
  | 'destructive'
  | 'success';

export interface RateLimitBadge {
  label: string;
  /** Badge variant from `@/shared/components/ui/badge`. */
  variant: RateLimitBadgeVariant;
  /** Long-form explanation for tooltips / sub-captions. */
  description: string;
}

export interface RateLimitState {
  /** A rate_limit_config is set on the connection. */
  hasConfig: boolean;
  /** Redis/Valkey is reachable, so a configured limit is actually enforced. */
  redisAvailable: boolean;
  /** The bucket is currently exhausted. */
  isRateLimited: boolean;
}

/**
 * Map a connection's live rate-limit state to a badge.
 *
 * Order matters — earlier conditions win:
 *  1. no config        → "No limit"      (neutral; requests are unthrottled)
 *  2. config, no redis → "Not enforced"  (configured but not applied right now)
 *  3. config, limited  → "Rate limited"  (at capacity)
 *  4. config, healthy  → "OK"            (enforced and within capacity)
 */
export function getRateLimitBadge(state: RateLimitState): RateLimitBadge {
  if (!state.hasConfig) {
    return {
      label: 'No limit',
      variant: 'muted',
      description:
        'No rate limit is configured — requests to this connection are not throttled.',
    };
  }
  if (!state.redisAvailable) {
    return {
      label: 'Not enforced',
      variant: 'warning',
      description:
        'A rate limit is configured but cannot be enforced right now (rate-limit tracking is unavailable).',
    };
  }
  if (state.isRateLimited) {
    return {
      label: 'Rate limited',
      variant: 'destructive',
      description: 'The rate limit is currently exhausted; requests are waiting.',
    };
  }
  return {
    label: 'OK',
    variant: 'success',
    description: 'A rate limit is configured and enforced, with capacity available.',
  };
}

/** Derive the badge from a {@link RateLimitStatusDto}. */
export function rateLimitBadgeFromStatus(
  status: RateLimitStatusDto
): RateLimitBadge {
  return getRateLimitBadge({
    hasConfig: status.config !== null && status.config !== undefined,
    redisAvailable: status.state.available,
    isRateLimited: status.metrics.isRateLimited,
  });
}
