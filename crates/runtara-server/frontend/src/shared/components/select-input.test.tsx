import { cleanup, render, screen } from '@testing-library/react';
import { useForm } from 'react-hook-form';
import { afterEach, beforeAll, describe, expect, it } from 'vitest';

import { SelectInput } from './select-input.tsx';
import { Form } from './ui/form.tsx';

const OPTIONS = [
  { value: 'wf-1', label: 'Orders sync' },
  { value: 'wf-2', label: 'Inventory reconcile' },
];

function Harness({ description }: { description?: string }) {
  const form = useForm({ defaultValues: { workflowId: '' } });

  return (
    <Form {...form}>
      <SelectInput
        name="workflowId"
        label="Workflow"
        placeholder="Select a workflow"
        description={description}
        options={OPTIONS}
      />
    </Form>
  );
}

afterEach(cleanup);

describe('SelectInput', () => {
  beforeAll(() => {
    // Radix's Select trigger measures itself; jsdom has no ResizeObserver.
    Object.defineProperty(globalThis, 'ResizeObserver', {
      writable: true,
      value: class {
        observe() {}
        unobserve() {}
        disconnect() {}
      },
    });
  });

  // FormControl has to wrap the trigger rather than the Radix root, or the
  // form item id lands on a component that renders no DOM and the label
  // associates with nothing.
  it('gives the trigger the form item id so the label names it', () => {
    render(<Harness />);

    const trigger = screen.getByRole('combobox', { name: 'Workflow' });
    const label = screen.getByText('Workflow');

    expect(trigger.id).not.toBe('');
    expect(label).toHaveAttribute('for', trigger.id);
  });

  it('describes the trigger with the field description', () => {
    render(<Harness description="Runs whenever the trigger fires." />);

    expect(
      screen.getByRole('combobox', { name: 'Workflow' })
    ).toHaveAccessibleDescription('Runs whenever the trigger fires.');
  });
});
