import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { RateLimitCard } from './index';
import type { RateLimitStatusDto } from '@/generated/RuntaraRuntimeApi';

const CONFIG = {
  requestsPerSecond: 20,
  burstSize: 40,
  retryOnLimit: true,
  maxRetries: 3,
  maxWaitMs: 60000,
};

function status(overrides: Partial<RateLimitStatusDto>): RateLimitStatusDto {
  return {
    connectionId: 'c1',
    connectionTitle: 'My Connection',
    integrationId: 'stripe_api_key',
    config: null,
    state: { available: true },
    metrics: { isRateLimited: false },
    periodStats: null,
    ...overrides,
  } as unknown as RateLimitStatusDto;
}

describe('RateLimitCard badge', () => {
  it('no config → "No limit" and never the green "OK" badge', () => {
    render(<RateLimitCard rateLimitStatus={status({ config: null })} />);
    expect(screen.getByText('No limit')).toBeInTheDocument();
    expect(screen.queryByText('OK')).not.toBeInTheDocument();
    expect(
      screen.getByText(/requests are not throttled/i)
    ).toBeInTheDocument();
  });

  it('config but Redis unavailable → "Not enforced"', () => {
    render(
      <RateLimitCard
        rateLimitStatus={status({
          config: CONFIG as RateLimitStatusDto['config'],
          state: { available: false } as RateLimitStatusDto['state'],
        })}
      />
    );
    expect(screen.getByText('Not enforced')).toBeInTheDocument();
    expect(screen.queryByText('OK')).not.toBeInTheDocument();
  });

  it('config, enforced, at capacity → "Rate limited"', () => {
    render(
      <RateLimitCard
        rateLimitStatus={status({
          config: CONFIG as RateLimitStatusDto['config'],
          state: { available: true } as RateLimitStatusDto['state'],
          metrics: { isRateLimited: true } as RateLimitStatusDto['metrics'],
        })}
      />
    );
    expect(screen.getByText('Rate limited')).toBeInTheDocument();
  });

  it('config, enforced, within capacity → "OK"', () => {
    render(
      <RateLimitCard
        rateLimitStatus={status({
          config: CONFIG as RateLimitStatusDto['config'],
        })}
      />
    );
    expect(screen.getByText('OK')).toBeInTheDocument();
  });
});
