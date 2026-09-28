import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { FormProvider, useForm } from 'react-hook-form';
import { NodeFormContext, NodeFormContextContextData } from './NodeFormContext';
import { SuspendingStepHint } from './SuspendingStepHint';
import { SuspendsBadge } from '@/features/workflows/components/CapabilityBadges';
import type { ExtendedAgent } from '@/features/workflows/queries';

const capability = (id: string, extra: Record<string, unknown> = {}) => ({
  id,
  name: id,
  inputType: 'Input',
  inputs: [],
  output: { type: 'object' },
  hasSideEffects: true,
  isIdempotent: false,
  rateLimited: false,
  ...extra,
});

const AGENTS = [
  {
    id: 'waiter',
    name: 'Waiter',
    description: '',
    supportsConnections: false,
    integrationIds: [],
    supportedCapabilities: {
      pause: capability('pause', { suspends: true }),
      plain: capability('plain'),
    },
  },
] as unknown as ExtendedAgent[];

function renderHint(values: Record<string, unknown>) {
  function Harness() {
    const form = useForm({
      defaultValues: { stepType: 'Agent', agentId: 'waiter', ...values },
    });
    return (
      <NodeFormContext.Provider
        value={
          {
            stepTypes: [],
            agents: AGENTS,
            workflows: [],
            executionGraph: null,
            isLoading: false,
            previousSteps: [],
          } as NodeFormContextContextData
        }
      >
        <FormProvider {...form}>
          <SuspendingStepHint />
        </FormProvider>
      </NodeFormContext.Provider>
    );
  }
  return render(<Harness />);
}

describe('suspending capability hint', () => {
  it('stays quiet for a capability that does not suspend', () => {
    renderHint({ capabilityId: 'plain' });
    expect(
      screen.queryByTestId('suspending-step-hint')
    ).not.toBeInTheDocument();
  });

  it('explains the requirements as a note when durable and timed out', () => {
    renderHint({ capabilityId: 'pause', durable: true, timeout: 60000 });
    const hint = screen.getByTestId('suspending-step-hint');
    expect(hint).toHaveAttribute('role', 'note');
    expect(hint).toHaveTextContent('durable');
    expect(screen.queryByText(/E028/)).not.toBeInTheDocument();
    expect(screen.queryByText(/E029/)).not.toBeInTheDocument();
  });

  it('highlights a missing timeout and durable off', () => {
    renderHint({ capabilityId: 'pause', durable: false, timeout: 0 });
    const hint = screen.getByTestId('suspending-step-hint');
    expect(hint).toHaveAttribute('role', 'alert');
    expect(screen.getByText(/Durable is off/)).toBeInTheDocument();
    expect(screen.getByText(/Timeout is missing or 0/)).toBeInTheDocument();
  });

  it('badges only suspending capabilities', () => {
    const { rerender } = render(
      <SuspendsBadge capability={{ suspends: true }} />
    );
    expect(screen.getByTestId('capability-suspends-badge')).toHaveTextContent(
      'Long-running'
    );
    rerender(<SuspendsBadge capability={{ suspends: false }} />);
    expect(
      screen.queryByTestId('capability-suspends-badge')
    ).not.toBeInTheDocument();
  });
});
