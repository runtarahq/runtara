import { render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { DebugStepInspector } from './DebugStepInspector';
import { useExecutionStore } from '@/features/workflows/stores/executionStore';
import { useWorkflowStore } from '@/features/workflows/stores/workflowStore';

vi.mock('sonner', () => ({
  toast: { success: vi.fn(), error: vi.fn(), info: vi.fn() },
}));

const initialWorkflow = useWorkflowStore.getState();
const initialExecution = useExecutionStore.getState();

function selectStep(
  id: string,
  debug: { inputs?: unknown; outputs?: unknown } | undefined,
  breakpoint?: { inputs: unknown }
) {
  useWorkflowStore.setState({
    selectedNodeId: id,
    nodes: [
      {
        id,
        position: { x: 0, y: 0 },
        data: { name: 'Fetch orders', stepType: 'Agent' },
      },
    ],
  } as never);
  useExecutionStore.setState({
    stepDebugData: new Map(debug ? [[id, debug]] : []),
    nodeExecutionStatus: new Map(),
    breakpointHit: breakpoint
      ? { stepId: id, inputs: breakpoint.inputs }
      : null,
  } as never);
}

afterEach(() => {
  useWorkflowStore.setState(initialWorkflow, true);
  useExecutionStore.setState(initialExecution, true);
});

describe('DebugStepInspector', () => {
  it('shows the selected step’s inputs and outputs in a compact explorer', () => {
    selectStep('fetch', {
      inputs: { limit: 500 },
      outputs: { orders: [{ id: 1 }] },
    });
    render(<DebugStepInspector />);
    const explorer = screen.getByRole('region', { name: 'Fetch orders data' });
    expect(screen.getByRole('tab', { name: /Inputs/ })).toHaveAttribute(
      'aria-selected',
      'true'
    );
    expect(explorer).toHaveTextContent('limit');
    expect(screen.getByRole('searchbox')).toHaveAttribute(
      'placeholder',
      'Search'
    );
  });

  it('labels breakpoint inputs as resolved', () => {
    selectStep('fetch', undefined, { inputs: { limit: 7 } });
    render(<DebugStepInspector />);
    expect(
      screen.getByRole('tab', { name: /Inputs \(resolved\)/ })
    ).toBeInTheDocument();
    expect(screen.getByRole('tab', { name: /Outputs/ })).toHaveTextContent('—');
  });

  it('says so when the step has no debug data', () => {
    selectStep('fetch', undefined);
    render(<DebugStepInspector />);
    expect(
      screen.queryByRole('region', { name: 'Fetch orders data' })
    ).toBeNull();
    expect(
      screen.getByText('No debug data available for this step')
    ).toBeInTheDocument();
  });
});
