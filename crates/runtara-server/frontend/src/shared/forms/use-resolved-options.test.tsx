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
  return <div>{JSON.stringify(state.options.resource ?? [])}</div>;
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

    rerender(<Harness company="second" resolver={resolver} />);
    await waitFor(() => expect(resolver).toHaveBeenCalledTimes(2));
    second.resolve([{ value: 'new', label: 'New' }]);
    await waitFor(() => expect(screen.getByText(/New/)).toBeInTheDocument());

    first.resolve([{ value: 'stale', label: 'Stale' }]);
    await Promise.resolve();
    expect(screen.queryByText(/Stale/)).not.toBeInTheDocument();
    expect(screen.getByText(/New/)).toBeInTheDocument();
  });
});
