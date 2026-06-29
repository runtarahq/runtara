import { describe, expect, it } from 'vitest';
import {
  getRateLimitBadge,
  rateLimitBadgeFromStatus,
} from './rate-limit-status';
import type { RateLimitStatusDto } from '@/generated/RuntaraRuntimeApi';

describe('getRateLimitBadge', () => {
  it('no config → neutral "No limit" (never the green success badge)', () => {
    // The unprotected state must NOT read as "OK" regardless of other flags.
    for (const redisAvailable of [true, false]) {
      for (const isRateLimited of [true, false]) {
        const badge = getRateLimitBadge({
          hasConfig: false,
          redisAvailable,
          isRateLimited,
        });
        expect(badge.label).toBe('No limit');
        expect(badge.variant).toBe('muted');
        expect(badge.variant).not.toBe('success');
      }
    }
  });

  it('config but Redis unavailable → "Not enforced" (warning)', () => {
    const badge = getRateLimitBadge({
      hasConfig: true,
      redisAvailable: false,
      isRateLimited: false,
    });
    expect(badge.label).toBe('Not enforced');
    expect(badge.variant).toBe('warning');
  });

  it('config, enforced, at capacity → "Rate limited" (destructive)', () => {
    const badge = getRateLimitBadge({
      hasConfig: true,
      redisAvailable: true,
      isRateLimited: true,
    });
    expect(badge.label).toBe('Rate limited');
    expect(badge.variant).toBe('destructive');
  });

  it('config, enforced, within capacity → "OK" (success)', () => {
    const badge = getRateLimitBadge({
      hasConfig: true,
      redisAvailable: true,
      isRateLimited: false,
    });
    expect(badge.label).toBe('OK');
    expect(badge.variant).toBe('success');
  });

  it('"Not enforced" wins over "Rate limited" when Redis is down', () => {
    const badge = getRateLimitBadge({
      hasConfig: true,
      redisAvailable: false,
      isRateLimited: true,
    });
    expect(badge.label).toBe('Not enforced');
  });
});

describe('rateLimitBadgeFromStatus', () => {
  const baseStatus = (
    overrides: Partial<RateLimitStatusDto>
  ): RateLimitStatusDto =>
    ({
      connectionId: 'c1',
      connectionTitle: 'Test',
      integrationId: 'stripe_api_key',
      config: null,
      state: { available: true },
      metrics: { isRateLimited: false },
      ...overrides,
    }) as unknown as RateLimitStatusDto;

  it('null config → "No limit"', () => {
    expect(rateLimitBadgeFromStatus(baseStatus({ config: null })).label).toBe(
      'No limit'
    );
  });

  it('present config + available redis → "OK"', () => {
    const status = baseStatus({
      config: {
        requestsPerSecond: 20,
        burstSize: 40,
        retryOnLimit: true,
        maxRetries: 3,
        maxWaitMs: 60000,
      },
      state: { available: true } as RateLimitStatusDto['state'],
      metrics: { isRateLimited: false } as RateLimitStatusDto['metrics'],
    });
    expect(rateLimitBadgeFromStatus(status).label).toBe('OK');
  });

  it('present config + redis down → "Not enforced"', () => {
    const status = baseStatus({
      config: {
        requestsPerSecond: 20,
        burstSize: 40,
        retryOnLimit: true,
        maxRetries: 3,
        maxWaitMs: 60000,
      },
      state: { available: false } as RateLimitStatusDto['state'],
    });
    expect(rateLimitBadgeFromStatus(status).label).toBe('Not enforced');
  });
});
