import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { ScenarioActionsForm } from './index';

const defaultProps = {
  isLoading: false,
  scenarioName: 'Test Scenario',
  onSchedule: vi.fn(),
  onSubmit: vi.fn(),
  onExportJSON: vi.fn(),
  onImportJSON: vi.fn(),
  onAutoLayout: vi.fn(),
  onAddNote: vi.fn(),
};

describe('ScenarioActionsForm', () => {
  describe('execution status text', () => {
    it('shows "Execution in progress" when execution is active (running)', () => {
      render(
        <ScenarioActionsForm
          {...defaultProps}
          isExecuting={true}
          isExecutionActive={true}
          executionStats={{ status: 'running', executionDuration: 5.12 }}
        />
      );

      expect(screen.getByText('Execution in progress')).toBeInTheDocument();
    });

    it('shows "Completed" when execution has completed', () => {
      render(
        <ScenarioActionsForm
          {...defaultProps}
          isExecuting={true}
          isExecutionActive={false}
          executionStats={{ status: 'completed', executionDuration: 20.05 }}
        />
      );

      expect(
        screen.queryByText('Execution in progress')
      ).not.toBeInTheDocument();
      expect(screen.getByText('Completed')).toBeInTheDocument();
    });

    it('shows "Execution failed" when execution has failed', () => {
      render(
        <ScenarioActionsForm
          {...defaultProps}
          isExecuting={true}
          isExecutionActive={false}
          executionStats={{ status: 'failed', executionDuration: 3.5 }}
        />
      );

      expect(
        screen.queryByText('Execution in progress')
      ).not.toBeInTheDocument();
      expect(screen.getByText('Execution failed')).toBeInTheDocument();
    });

    it('shows "Execution timed out" when execution has timed out', () => {
      render(
        <ScenarioActionsForm
          {...defaultProps}
          isExecuting={true}
          isExecutionActive={false}
          executionStats={{ status: 'timeout', executionDuration: 60 }}
        />
      );

      expect(screen.getByText('Execution timed out')).toBeInTheDocument();
    });

    it('shows "Execution cancelled" when execution was cancelled', () => {
      render(
        <ScenarioActionsForm
          {...defaultProps}
          isExecuting={true}
          isExecutionActive={false}
          executionStats={{ status: 'cancelled', executionDuration: 2.1 }}
        />
      );

      expect(screen.getByText('Execution cancelled')).toBeInTheDocument();
    });

    it('does not show execution status when not executing', () => {
      render(
        <ScenarioActionsForm
          {...defaultProps}
          isExecuting={false}
          isExecutionActive={false}
        />
      );

      expect(
        screen.queryByText('Execution in progress')
      ).not.toBeInTheDocument();
      expect(screen.queryByText('Completed')).not.toBeInTheDocument();
    });
  });
});
