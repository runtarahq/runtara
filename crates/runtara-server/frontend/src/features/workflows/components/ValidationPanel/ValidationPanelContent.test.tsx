import { describe, expect, it, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useValidationStore } from '../../stores/validationStore';
import { ValidationPanelContent } from './ValidationPanelContent';
import { ValidationMessage } from '../../types/validation';

const mockMessages: ValidationMessage[] = [
  {
    id: 'err-1',
    severity: 'error',
    code: 'E001',
    message: 'Step is not connected to the workflow',
    stepId: 'step-1',
    stepName: 'HTTP Request',
    source: 'client',
    timestamp: 1,
  },
  {
    id: 'err-2',
    severity: 'error',
    code: 'E023',
    message: 'Type mismatch in input field',
    stepId: 'step-2',
    stepName: 'Conditional',
    source: 'server',
    timestamp: 2,
  },
  {
    id: 'warn-1',
    severity: 'warning',
    code: 'W001',
    message: 'Step without description: HTTP Request',
    stepId: 'step-1',
    stepName: 'HTTP Request',
    source: 'client',
    timestamp: 3,
  },
];

describe('ValidationPanelContent — filter tab switching (SYN-235)', () => {
  beforeEach(() => {
    useValidationStore.setState({
      messages: [],
      stepsWithErrors: new Set<string>(),
      stepsWithWarnings: new Set<string>(),
      isPanelExpanded: false,
      activeTab: 'problems',
      activeFilter: 'all',
      lastValidatedAt: null,
    });
  });

  it('shows all messages on the All tab', () => {
    useValidationStore.getState().setMessages(mockMessages);
    render(<ValidationPanelContent onNavigateToStep={() => {}} />);

    expect(
      screen.getByText('Step is not connected to the workflow')
    ).toBeInTheDocument();
    expect(
      screen.getByText('Type mismatch in input field')
    ).toBeInTheDocument();
    expect(
      screen.getByText('Step without description: HTTP Request')
    ).toBeInTheDocument();
  });

  it('shows only errors after clicking Errors tab', async () => {
    const user = userEvent.setup();
    useValidationStore.getState().setMessages(mockMessages);
    render(<ValidationPanelContent onNavigateToStep={() => {}} />);

    await user.click(screen.getByRole('button', { name: /Errors \(2\)/ }));

    expect(
      screen.getByText('Step is not connected to the workflow')
    ).toBeInTheDocument();
    expect(
      screen.getByText('Type mismatch in input field')
    ).toBeInTheDocument();
    expect(
      screen.queryByText('Step without description: HTTP Request')
    ).not.toBeInTheDocument();
  });

  it('shows only warnings after clicking Warnings tab', async () => {
    const user = userEvent.setup();
    useValidationStore.getState().setMessages(mockMessages);
    render(<ValidationPanelContent onNavigateToStep={() => {}} />);

    await user.click(screen.getByRole('button', { name: /Warnings \(1\)/ }));

    expect(
      screen.queryByText('Step is not connected to the workflow')
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText('Type mismatch in input field')
    ).not.toBeInTheDocument();
    expect(
      screen.getByText('Step without description: HTTP Request')
    ).toBeInTheDocument();
  });

  it('updates displayed messages when switching between tabs', async () => {
    const user = userEvent.setup();
    useValidationStore.getState().setMessages(mockMessages);
    render(<ValidationPanelContent onNavigateToStep={() => {}} />);

    // Start on All — 3 messages
    expect(screen.getAllByText(/\[E|W\d+\]/)).toHaveLength(3);

    // Switch to Errors — 2 messages
    await user.click(screen.getByRole('button', { name: /Errors \(2\)/ }));
    expect(screen.getAllByText(/\[E\d+\]/)).toHaveLength(2);
    expect(screen.queryByText(/\[W\d+\]/)).not.toBeInTheDocument();

    // Switch to Warnings — 1 message
    await user.click(screen.getByRole('button', { name: /Warnings \(1\)/ }));
    expect(screen.getAllByText(/\[W\d+\]/)).toHaveLength(1);
    expect(screen.queryByText(/\[E\d+\]/)).not.toBeInTheDocument();

    // Switch back to All — 3 messages
    await user.click(screen.getByRole('button', { name: /All \(3\)/ }));
    expect(screen.getAllByText(/\[E|W\d+\]/)).toHaveLength(3);
  });

  it('shows empty state when no messages match filter', async () => {
    const user = userEvent.setup();
    // Set only errors, no warnings
    useValidationStore
      .getState()
      .setMessages([mockMessages[0], mockMessages[1]]);
    render(<ValidationPanelContent onNavigateToStep={() => {}} />);

    // Switch to Warnings — should show empty state
    await user.click(screen.getByRole('button', { name: /Warnings \(0\)/ }));
    expect(screen.getByText('No issues in this category')).toBeInTheDocument();
  });
});
