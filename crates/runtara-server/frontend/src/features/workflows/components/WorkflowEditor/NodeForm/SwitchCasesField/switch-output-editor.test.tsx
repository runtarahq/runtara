import { useState } from 'react';
import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { TooltipProvider } from '@/shared/components/ui/tooltip';
import { ObjectMappingEditor } from '../InputMappingField/ObjectMappingEditor';
import { NodeFormContext } from '../NodeFormContext';
import { readSwitchOutput, writeSwitchOutput } from './switch-output';

// The real picker lists the workflow's paths; this one offers a single path.
vi.mock('../InputMappingField/VariablePickerModal', () => ({
  VariablePickerModal: ({
    open,
    onSelect,
  }: {
    open: boolean;
    onSelect: (variable: { value: string }) => void;
  }) =>
    open ? (
      <button
        type="button"
        onClick={() => onSelect({ value: "steps['fetch'].outputs.count" })}
      >
        pick fetch count
      </button>
    ) : null,
}));

const ctx = {
  previousSteps: [],
  inputSchemaFields: [],
  variables: [],
  isInsideSplit: false,
  isInsideWaitScope: false,
  splitItemSchemaFields: [],
  nodeId: 'route',
  agents: [],
  workflows: [],
  stepTypes: [],
};

/** A composite case output, edited the way SwitchOutputCell wires it. */
function renderCompositeOutput(stored: unknown) {
  const saved: { current: unknown } = { current: stored };
  function Harness() {
    const [output, setOutput] = useState<unknown>(stored);
    return (
      <ObjectMappingEditor
        value={readSwitchOutput(output).value as never}
        valueType="composite"
        untyped
        onChange={(next) => {
          saved.current = writeSwitchOutput({ mode: 'composite', value: next });
          setOutput(saved.current);
        }}
        onValueTypeChange={vi.fn()}
        onClose={vi.fn()}
      />
    );
  }
  render(
    <TooltipProvider>
      <NodeFormContext.Provider value={ctx as never}>
        <Harness />
      </NodeFormContext.Provider>
    </TooltipProvider>
  );
  return saved;
}

describe('Switch composite output editing', () => {
  it("keeps a nested reference's type and default when its path is replaced", () => {
    const saved = renderCompositeOutput({
      count: {
        valueType: 'reference',
        value: 'data.n',
        type: 'integer',
        default: 0,
      },
    });

    fireEvent.click(screen.getByLabelText('Remove reference'));
    expect(saved.current).toEqual({
      count: { valueType: 'reference', value: '', type: 'integer', default: 0 },
    });

    fireEvent.click(screen.getByText('Click to select variable...'));
    fireEvent.click(screen.getByText('pick fetch count'));
    expect(saved.current).toEqual({
      count: {
        valueType: 'reference',
        value: "steps['fetch'].outputs.count",
        type: 'integer',
        default: 0,
      },
    });
  });
});
