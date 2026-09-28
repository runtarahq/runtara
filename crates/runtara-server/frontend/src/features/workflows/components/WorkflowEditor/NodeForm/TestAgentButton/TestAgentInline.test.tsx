import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { FormProvider, useForm } from 'react-hook-form';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import {
  NodeFormContext,
  NodeFormContextContextData,
} from '../NodeFormContext';
import { getTestHandler, TestAgentInline } from './TestAgentInline';
import { REQUIRES_RUN_TEST_HINT } from '@/features/workflows/utils/capability-flags';
import type { ExtendedAgent } from '@/features/workflows/queries';

vi.mock('@/shared/hooks', () => ({ useToken: () => 'token' }));
vi.mock('@/shared/hooks/useEntitlements', () => ({
  useEntitlements: () => null,
}));
vi.mock('@/shared/entitlements', () => ({ agentEnabled: () => true }));
vi.mock('../InputMappingField/SimpleInputMappingEditor', () => ({
  SimpleInputMappingEditor: () => <div>test inputs</div>,
}));

const capability = (id: string, tags: string[]) => ({
  id,
  name: id,
  inputType: 'Input',
  inputs: [],
  output: { type: 'object' },
  hasSideEffects: true,
  isIdempotent: false,
  rateLimited: false,
  tags,
});

const AGENTS = [
  {
    id: 'control',
    name: 'Control',
    description: '',
    supportsConnections: false,
    integrationIds: [],
    supportedCapabilities: {
      start: capability('start', ['runtime:requires-run']),
    },
  },
  {
    id: 'utils',
    name: 'Utils',
    description: '',
    supportsConnections: false,
    integrationIds: [],
    supportedCapabilities: { echo: capability('echo', []) },
  },
] as unknown as ExtendedAgent[];

function renderTest(agentId: string, capabilityId: string) {
  function Harness() {
    const form = useForm({
      defaultValues: { stepType: 'Agent', agentId, capabilityId },
    });
    return (
      <QueryClientProvider client={new QueryClient()}>
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
            <TestAgentInline />
          </FormProvider>
        </NodeFormContext.Provider>
      </QueryClientProvider>
    );
  }
  return render(<Harness />);
}

describe('capability Test for run-only capabilities', () => {
  it('disables Test and explains a runtime:requires-run capability', () => {
    renderTest('control', 'start');
    expect(screen.getByTestId('test-requires-run')).toHaveTextContent(
      REQUIRES_RUN_TEST_HINT
    );
    expect(getTestHandler()).toMatchObject({
      isAvailable: false,
      unavailableReason: REQUIRES_RUN_TEST_HINT,
    });
  });

  it('keeps Test available for an ordinary capability', () => {
    renderTest('utils', 'echo');
    expect(screen.queryByTestId('test-requires-run')).not.toBeInTheDocument();
    expect(getTestHandler()).toMatchObject({ isAvailable: true });
    expect(getTestHandler()?.unavailableReason).toBeUndefined();
  });
});
