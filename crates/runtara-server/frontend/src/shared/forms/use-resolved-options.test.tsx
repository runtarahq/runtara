import { render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import type { FormDefinition, FormOption, OptionResolver } from './types';
import { useResolvedOptions } from './use-resolved-options';

const definition: FormDefinition = {
  fields: {
    company: { type: 'string' },
    resource: {
      type: 'string',
      control: {
        kind: 'lookup',
        optionResolver: 'resources',
        optionDependencies: ['company'],
      },
    },
  },
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((next) => {
    resolve = next;
  });
  return { promise, resolve };
}

function Harness({
  company,
  resolver,
}: {
  company: string;
  resolver: OptionResolver;
}) {
  const state = useResolvedOptions(definition, { company }, resolver);
  return (
    <div>
      <span data-testid="loading">
        {state.loading.has('resource') ? 'loading' : 'idle'}
      </span>
      <span data-testid="error">{state.errors.resource ?? ''}</span>
      <span>{JSON.stringify(state.options.resource ?? [])}</span>
    </div>
  );
}

describe('useResolvedOptions', () => {
  it('ignores stale responses after a declared dependency changes', async () => {
    const first = deferred<FormOption[]>();
    const second = deferred<FormOption[]>();
    const resolver = vi
      .fn()
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise);
    const { rerender } = render(
      <Harness company="first" resolver={resolver} />
    );
    await waitFor(() => expect(resolver).toHaveBeenCalledTimes(1));
    const firstSignal = resolver.mock.calls[0][0].signal as AbortSignal;
    expect(screen.getByTestId('loading')).toHaveTextContent('loading');

    rerender(<Harness company="second" resolver={resolver} />);
    await waitFor(() => expect(resolver).toHaveBeenCalledTimes(2));
    expect(firstSignal.aborted).toBe(true);
    second.resolve([{ value: 'new', label: 'New' }]);
    await waitFor(() => expect(screen.getByText(/New/)).toBeInTheDocument());

    first.resolve([{ value: 'stale', label: 'Stale' }]);
    await Promise.resolve();
    expect(screen.queryByText(/Stale/)).not.toBeInTheDocument();
    expect(screen.getByText(/New/)).toBeInTheDocument();
    expect(screen.getByTestId('loading')).toHaveTextContent('idle');
  });

  it('surfaces resolver failures and clears the loading state', async () => {
    const resolver = vi.fn().mockRejectedValue(new Error('Provider unavailable'));
    render(<Harness company="acme" resolver={resolver} />);

    expect(await screen.findByText('Provider unavailable')).toBeInTheDocument();
    expect(screen.getByTestId('loading')).toHaveTextContent('idle');
    expect(screen.queryByText(/loading/)).not.toBeInTheDocument();
  });

  it('replaces the option set when dependencies invalidate earlier choices', async () => {
    const resolver = vi
      .fn()
      .mockResolvedValueOnce([{ value: 'old', label: 'Old choice' }])
      .mockResolvedValueOnce([{ value: 'new', label: 'New choice' }]);
    const { rerender } = render(
      <Harness company="first" resolver={resolver} />
    );
    expect(await screen.findByText(/Old choice/)).toBeInTheDocument();

    rerender(<Harness company="second" resolver={resolver} />);
    expect(await screen.findByText(/New choice/)).toBeInTheDocument();
    expect(screen.queryByText(/Old choice/)).not.toBeInTheDocument();
  });
});
