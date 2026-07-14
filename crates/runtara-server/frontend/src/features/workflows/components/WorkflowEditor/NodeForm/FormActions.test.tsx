import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import type { ReactNode } from 'react';
import type { SchemaType } from './NodeFormItem';

// FormActions reads the active tab from useTabContext and the current test
// handler from the TestAgentInline singleton. Mock both so we can drive the
// active tab and assert rendering without the heavy NodeForm field tree.
// NodeFormItem is also stubbed down to the few form.* values NodeForm consumes
// at render time, so the NodeForm wiring tests below stay lightweight.
const mocks = vi.hoisted(() => ({
  activeTab: 'main' as 'main' | 'testing',
  testHandler: null as {
    runTest: () => void;
    isPending: boolean;
    isValid: boolean;
    isAvailable: boolean;
  } | null,
}));

vi.mock('./NodeFormItem.tsx', async () => {
  const { z } = await import('zod');
  return {
    useTabContext: () => ({
      activeTab: mocks.activeTab,
      setActiveTab: () => {},
    }),
    TabProvider: ({ children }: { children: ReactNode }) => children,
    schema: () => z.object({}).passthrough(),
    initialValues: {},
    fieldsConfig: [],
  };
});

vi.mock('./TestAgentButton/TestAgentInline', () => ({
  getTestHandler: () => mocks.testHandler,
}));

import { FormActions, NodeForm } from './index';

const noop = () => {};

const availableHandler = (
  overrides: Partial<NonNullable<typeof mocks.testHandler>> = {}
) => ({
  runTest: noop,
  isPending: false,
  isValid: true,
  isAvailable: true,
  ...overrides,
});

afterEach(() => {
  cleanup();
  mocks.activeTab = 'main';
  mocks.testHandler = null;
});

// The regression guard for SYN-548. The create flow (TimelineNodeConfigPanel
// with isCreate) renders NodeForm with hideActions=true because the panel
// supplies its own Save/Cancel footer. That must NOT drop the Run Test action,
// which is testing-tab-only and provided by no parent footer.
describe('NodeForm wires the testing-tab Run Test action', () => {
  const renderNodeForm = (hideActions: boolean) =>
    render(
      <NodeForm
        values={{} as SchemaType}
        onSubmit={noop}
        hideActions={hideActions}
      />
    );

  it('shows Run Test on the testing tab even when hideActions is set', () => {
    mocks.activeTab = 'testing';
    mocks.testHandler = availableHandler();

    renderNodeForm(true);

    expect(screen.getByTestId('node-form-run-test')).toBeInTheDocument();
  });

  it('still shows Run Test on the testing tab when actions are not hidden', () => {
    mocks.activeTab = 'testing';
    mocks.testHandler = availableHandler();

    renderNodeForm(false);

    expect(screen.getByTestId('node-form-run-test')).toBeInTheDocument();
  });

  it('suppresses the main-tab Save when hideActions is set (no duplicate)', () => {
    mocks.activeTab = 'main';

    renderNodeForm(true);

    expect(
      screen.queryByTestId('node-form-create-save')
    ).not.toBeInTheDocument();
  });

  it('shows the create Save on the main tab when actions are not hidden', () => {
    mocks.activeTab = 'main';

    renderNodeForm(false);

    expect(screen.getByTestId('node-form-create-save')).toBeInTheDocument();
  });
});

// Focused checks of FormActions' own branch logic.
describe('FormActions', () => {
  it('invokes the handler when Run Test is clicked (no saved step required)', () => {
    const runTest = vi.fn();
    mocks.activeTab = 'testing';
    mocks.testHandler = availableHandler({ runTest });

    render(<FormActions hideActions onReset={noop} onDelete={noop} />);
    fireEvent.click(screen.getByTestId('node-form-run-test'));

    expect(runTest).toHaveBeenCalledTimes(1);
  });

  it('disables Run Test until the handler reports it is valid', () => {
    mocks.activeTab = 'testing';
    mocks.testHandler = availableHandler({ isValid: false });

    render(<FormActions hideActions onReset={noop} onDelete={noop} />);

    expect(screen.getByTestId('node-form-run-test')).toBeDisabled();
  });

  it('shows Delete and Reset on the main tab in edit mode', () => {
    mocks.activeTab = 'main';

    render(<FormActions isEdit onReset={noop} onDelete={noop} />);

    expect(screen.getByTestId('node-form-delete')).toBeInTheDocument();
    expect(screen.getByTestId('node-form-reset')).toBeInTheDocument();
  });
});
