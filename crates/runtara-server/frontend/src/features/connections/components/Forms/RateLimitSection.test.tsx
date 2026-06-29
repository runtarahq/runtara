import { describe, expect, it } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { FormProvider, useForm } from 'react-hook-form';
import { MemoryRouter } from 'react-router';
import { RateLimitSection } from './RateLimitSection';
import type { RateLimitConfigDto } from '@/generated/RuntaraRuntimeApi';

function Wrapper({ children }: { children: React.ReactNode }) {
  const form = useForm({
    defaultValues: {
      rateLimitEnabled: false,
      requestsPerSecond: '',
      burstSize: '',
      maxRetries: '',
      maxWaitMs: '',
      retryOnLimit: true,
    },
  });
  return (
    <MemoryRouter>
      <FormProvider {...form}>{children}</FormProvider>
    </MemoryRouter>
  );
}

const DEFAULT_CONFIG: RateLimitConfigDto = {
  requestsPerSecond: 2,
  burstSize: 4,
  retryOnLimit: true,
  maxRetries: 3,
  maxWaitMs: 60000,
};

describe('RateLimitSection', () => {
  it('no default → states "unlimited" plainly with an "Enable rate limiting" label', () => {
    render(
      <Wrapper>
        <RateLimitSection />
      </Wrapper>
    );
    expect(screen.getByText(/No rate limiting is applied/i)).toBeInTheDocument();
    expect(screen.getByText(/requests to this connection are unlimited/i))
      .toBeInTheDocument();
    expect(screen.getByText('Enable rate limiting')).toBeInTheDocument();
    // The default form must NOT imply a baseline is being declined.
    expect(
      screen.queryByText('Override default rate limits')
    ).not.toBeInTheDocument();
  });

  it('with a default → uses "Override default rate limits" label', () => {
    render(
      <Wrapper>
        <RateLimitSection defaultConfig={DEFAULT_CONFIG} />
      </Wrapper>
    );
    expect(
      screen.getByText('Override default rate limits')
    ).toBeInTheDocument();
    expect(
      screen.queryByText(/No rate limiting is applied/i)
    ).not.toBeInTheDocument();
  });

  it('"Set a safe limit" enables the override and reveals the config inputs', () => {
    render(
      <Wrapper>
        <RateLimitSection />
      </Wrapper>
    );
    // Grid is hidden until enabled.
    expect(screen.queryByText('Burst size')).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /set a safe limit/i }));

    // Now the rate-limit config inputs are visible.
    expect(screen.getByText('Burst size')).toBeInTheDocument();
    expect(screen.getByText('Requests per second')).toBeInTheDocument();
  });

  it('renders a cross-link to the analytics rate-limits page', () => {
    render(
      <Wrapper>
        <RateLimitSection />
      </Wrapper>
    );
    const link = screen.getByRole('link', {
      name: /view live rate-limit activity/i,
    });
    expect(link).toHaveAttribute('href', '/analytics/rate-limits');
  });
});
