import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { FormProvider, useForm } from 'react-hook-form';
import { NodeFormContext, NodeFormContextContextData } from './NodeFormContext';
import { StepOutputPanel } from './StepOutputPanel';
import {
  __resetStepOutputShapesForTests,
  __setStepOutputShapesForTests,
} from '@/features/workflows/utils/step-output-shapes';
import type { ExtendedAgent } from '@/features/workflows/queries';

const AGENTS = [
  {
    id: 'http',
    name: 'HTTP',
    description: '',
    supportsConnections: false,
    integrationIds: [],
    supportedCapabilities: {
      'http-request': {
        id: 'http-request',
        name: 'http_request',
        inputType: 'HttpRequestInput',
        inputs: [],
        output: {
          type: 'object',
          fields: [
            { name: 'status_code', type: 'integer' },
            {
              name: 'body',
              type: 'object',
              fields: [{ name: 'token', type: 'string' }],
            },
          ],
        },
        hasSideEffects: true,
        isIdempotent: false,
        rateLimited: false,
      },
    },
  },
  {
    id: 'xlsx',
    name: 'XLSX',
    description: '',
    supportsConnections: false,
    integrationIds: [],
    supportedCapabilities: {
      'get-sheets': {
        id: 'get-sheets',
        name: 'get_sheets',
        inputType: 'GetSheetsInput',
        inputs: [],
        output: {
          type: 'array',
          items: {
            type: 'object',
            fields: [
              { name: 'name', type: 'string' },
              { name: 'index', type: 'integer' },
            ],
          },
        },
        hasSideEffects: false,
        isIdempotent: true,
        rateLimited: false,
      },
    },
  },
] as unknown as ExtendedAgent[];

function renderPanel(
  defaultValues: Record<string, unknown>,
  context: Partial<NodeFormContextContextData> = {}
) {
  function Harness() {
    const form = useForm({ defaultValues });
    return (
      <NodeFormContext.Provider
        value={
          {
            nodeId: 'fetch',
            stepTypes: [],
            agents: AGENTS,
            workflows: [],
            executionGraph: null,
            isLoading: false,
            previousSteps: [],
            ...context,
          } as NodeFormContextContextData
        }
      >
        <FormProvider {...form}>
          <StepOutputPanel />
        </FormProvider>
      </NodeFormContext.Provider>
    );
  }
  return render(<Harness />);
}

function openPanel() {
  fireEvent.click(screen.getByTestId('step-output-panel-trigger'));
}

describe('StepOutputPanel', () => {
  beforeEach(() => {
    __setStepOutputShapesForTests({
      Split: {
        summary:
          '`outputs` is the array of successful per-item subgraph outputs.',
        outputs: { kind: 'array' },
        siblingFields: [
          { name: 'stats', type: 'object', description: 'Per-outcome counts' },
          { name: 'hasFailures', type: 'boolean' },
        ],
      },
      Filter: {
        outputs: {
          kind: 'object',
          fields: [
            { name: 'items', type: 'array', description: 'Kept items' },
            { name: 'count', type: 'integer' },
          ],
        },
        siblingFields: [],
      },
    });
  });

  afterEach(() => {
    __resetStepOutputShapesForTests();
  });

  it('renders capability output fields with types for Agent steps', () => {
    renderPanel({
      stepType: 'Agent',
      agentId: 'http',
      capabilityId: 'http-request',
    });
    openPanel();

    expect(screen.getByText('status_code')).toBeInTheDocument();
    expect(screen.getByText('integer')).toBeInTheDocument();
    // Nested fields render indented rows.
    expect(screen.getByText('token')).toBeInTheDocument();
    expect(screen.getByText('steps.fetch.outputs')).toBeInTheDocument();
  });

  it('marks array-output item fields as per-element, not direct paths', () => {
    renderPanel({
      stepType: 'Agent',
      agentId: 'xlsx',
      capabilityId: 'get-sheets',
    });
    openPanel();

    // Item fields must not read as steps.<id>.outputs.name — the value is an
    // array and that path resolves to null.
    expect(screen.getByText('[item].name')).toBeInTheDocument();
    expect(screen.getByText('[item].index')).toBeInTheDocument();
    expect(screen.getByText(/address by index/)).toBeInTheDocument();
    expect(screen.queryByText(/^name$/)).not.toBeInTheDocument();
  });

  it('renders the canonical shape for control steps, including siblings', () => {
    renderPanel({ stepType: 'Split' });
    openPanel();

    expect(
      screen.getByText(/array of successful per-item subgraph outputs/)
    ).toBeInTheDocument();
    expect(screen.getByText('stats')).toBeInTheDocument();
    expect(screen.getByText('hasFailures')).toBeInTheDocument();
    expect(screen.getByText('boolean')).toBeInTheDocument();
  });

  it('renders typed object fields for Filter', () => {
    renderPanel({ stepType: 'Filter' });
    openPanel();

    expect(screen.getByText('items')).toBeInTheDocument();
    expect(screen.getByText('count')).toBeInTheDocument();
    expect(screen.getByText('Kept items')).toBeInTheDocument();
  });

  it('is hidden for Finish steps and capability-less Agent steps', () => {
    const finish = renderPanel({ stepType: 'Finish' });
    expect(
      finish.queryByTestId('step-output-panel-trigger')
    ).not.toBeInTheDocument();
    finish.unmount();

    const bareAgent = renderPanel({ stepType: 'Agent' });
    expect(
      bareAgent.queryByTestId('step-output-panel-trigger')
    ).not.toBeInTheDocument();
  });
});
