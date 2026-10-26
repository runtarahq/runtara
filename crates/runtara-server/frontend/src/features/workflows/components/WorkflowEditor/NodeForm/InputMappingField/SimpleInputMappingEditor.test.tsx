import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';

import type { CapabilityField } from '@/generated/RuntaraRuntimeApi';
import { useNodeFormStore } from '@/features/workflows/stores/nodeFormStore';
import { TooltipProvider } from '@/shared/components/ui/tooltip';
import {
  composeExecutionGraph,
  executionGraphToReactFlow,
} from '../../CustomNodes/utils';
import { NodeFormContext } from '../NodeFormContext';
import { SimpleInputMappingEditor } from './SimpleInputMappingEditor';
import { toEditorInitialData, toFormMappingEntries } from './mapping-entries';

/**
 * An Agent input stored as an immediate whose value is a structure — authored
 * via MCP or the REST API — opened as a Build with one "Unknown type" per key,
 * and the first edit there rewrote it as a composite, which the server rejects
 * for a condition input. These tests open the step the way the canvas does
 * (load → panel → save) and check both what the panel shows and what a save
 * writes back.
 */

// The Object Model query condition from the report: argument 0 names a
// column, argument 1 reads run data.
const CONDITION = {
  type: 'operation',
  op: 'EQ',
  arguments: [
    { valueType: 'reference', value: 'name' },
    { valueType: 'reference', value: 'data.body.supplier' },
  ],
};

// object-model:query-instances inputs, plus an array-typed one.
const FIELDS = [
  { name: 'schema_name', type: 'string', required: true },
  { name: 'filters', type: 'object', required: false },
  { name: 'condition', type: 'any', required: false },
  { name: 'fields', type: 'array<string>', required: false },
] as unknown as CapabilityField[];

const STEP_ID = 'query';

const CONTEXT = {
  previousSteps: [],
  inputSchemaFields: [],
  variables: [],
  isInsideSplit: false,
  isInsideWaitScope: false,
  splitItemSchemaFields: [],
  nodeId: STEP_ID,
  agents: [],
  workflows: [],
  stepTypes: [],
};

let openedNodeIds: string[] = [];

afterEach(() => {
  openedNodeIds.forEach((id) => useNodeFormStore.getState().clearNode(id));
  openedNodeIds = [];
});

/** Open a query step holding `inputMapping` in the panel. */
function openStep(
  inputMapping: Record<string, unknown>,
  fields: CapabilityField[] = FIELDS
) {
  const graph = {
    name: 'syn-fixture',
    steps: {
      [STEP_ID]: {
        id: STEP_ID,
        stepType: 'Agent',
        agentId: 'object-model',
        capabilityId: 'query-instances',
        inputMapping,
        renderingParameters: { x: 0, y: 0 },
      },
    },
    executionPlan: [],
    entryPoint: STEP_ID,
  };
  const { nodes, edges } = executionGraphToReactFlow(graph as never);
  const node = nodes[0];
  openedNodeIds.push(node.id);

  // The app mounts a TooltipProvider at its root; the Build editor needs it.
  render(
    <TooltipProvider>
      <NodeFormContext.Provider value={CONTEXT as never}>
        <SimpleInputMappingEditor
          nodeId={node.id}
          fields={fields}
          initialData={toEditorInitialData(node.data.inputMapping as never)}
        />
      </NodeFormContext.Provider>
    </TooltipProvider>
  );

  /** What saving the panel writes to the step's `inputMapping`. */
  const save = (): Record<string, unknown> => {
    const entries = useNodeFormStore.getState().getNodeInputMapping(node.id);
    const saved = composeExecutionGraph(
      [
        {
          ...node,
          data: { ...node.data, inputMapping: toFormMappingEntries(entries) },
        },
      ],
      edges,
      { name: graph.name }
    );
    return (saved!.steps as Record<string, any>)[STEP_ID].inputMapping;
  };
  const entry = (field: string) =>
    useNodeFormStore.getState().getFieldEntry(node.id, field);

  return { save, entry };
}

/** Open a field's inline editor from its collapsed row. */
function expand(summary: string | RegExp) {
  fireEvent.click(screen.getByText(summary));
}

/** A segment of the open editor's mode toggle (the table header says "Value" too). */
function modeButton(name: 'Value' | 'Reference' | 'Build') {
  return screen.getByRole('button', { name });
}

function jsonBox(): HTMLTextAreaElement {
  return screen.getByPlaceholderText(
    'Enter a JSON value...'
  ) as HTMLTextAreaElement;
}

describe('SimpleInputMappingEditor — immediate condition on an `any` input', () => {
  const STORED = {
    schema_name: { valueType: 'immediate', value: 'suppliers' },
    condition: { valueType: 'immediate', value: CONDITION },
  };

  it('reads as the condition, not as a Build of unknown types', () => {
    openStep(STORED);
    // Collapsed, the row reads like the canvas card.
    expand('name is Input · body.supplier');

    expect(screen.queryByText('Unknown type')).toBeNull();
    expect(screen.queryByText(/Composite Object/)).toBeNull();
    expect(jsonBox().value).toBe(JSON.stringify(CONDITION, null, 2));
  });

  it('saves an untouched condition exactly as it was stored', () => {
    const { save } = openStep(STORED);
    expand('name is Input · body.supplier');

    expect(save()).toEqual(STORED);
  });

  it('saves an edited condition as an immediate, never a composite', () => {
    const { save, entry } = openStep(STORED);
    expand('name is Input · body.supplier');

    const edited = { ...CONDITION, op: 'NE' };
    const typed = JSON.stringify(edited);
    fireEvent.change(jsonBox(), { target: { value: typed } });

    // Valid JSON is stored as the structure it describes...
    expect(entry('condition')).toMatchObject({
      valueType: 'immediate',
      value: edited,
    });
    // ...while the box keeps the text as typed rather than re-printing it.
    expect(jsonBox().value).toBe(typed);
    expect(save().condition).toEqual({ valueType: 'immediate', value: edited });
    expect(screen.getByText('name is not Input · body.supplier')).toBeTruthy();
  });

  it('saves a literal holding {{ as the structure, not as text', () => {
    // The save path leaves text containing `{{` unparsed, so a literal held
    // as text would have been saved as a string.
    const { save } = openStep(STORED);
    expand('name is Input · body.supplier');

    const edited = {
      ...CONDITION,
      arguments: [
        { valueType: 'reference', value: 'name' },
        { valueType: 'immediate', value: 'Hello {{name}}' },
      ],
    };
    fireEvent.change(jsonBox(), {
      target: { value: JSON.stringify(edited, null, 2) },
    });

    expect(save().condition).toEqual({ valueType: 'immediate', value: edited });
  });

  it('holds text that does not parse yet under the json hint', () => {
    const { save, entry } = openStep(STORED);
    expand('name is Input · body.supplier');

    fireEvent.change(jsonBox(), { target: { value: '{"type": "operation",' } });
    // The hint is what makes the node form flag it and the save parse it.
    expect(entry('condition')).toMatchObject({
      valueType: 'immediate',
      value: '{"type": "operation",',
      typeHint: 'json',
    });

    fireEvent.change(jsonBox(), {
      target: { value: JSON.stringify(CONDITION) },
    });
    expect(save().condition).toEqual({
      valueType: 'immediate',
      value: CONDITION,
    });
  });

  it('keeps the JSON editor while the text is cleared and retyped', () => {
    const { save } = openStep(STORED);
    expand('name is Input · body.supplier');

    fireEvent.change(jsonBox(), { target: { value: '' } });
    fireEvent.change(jsonBox(), {
      target: { value: JSON.stringify({ ...CONDITION, op: 'GT' }) },
    });

    expect(save().condition).toEqual({
      valueType: 'immediate',
      value: { ...CONDITION, op: 'GT' },
    });
  });

  it('does not wipe the condition when the lit Value segment is clicked', () => {
    const { save } = openStep(STORED);
    expand('name is Input · body.supplier');

    fireEvent.click(modeButton('Value'));

    expect(jsonBox().value).toBe(JSON.stringify(CONDITION, null, 2));
    expect(save()).toEqual(STORED);
  });

  it('leaves an emptied condition out of the save', () => {
    const { save } = openStep(STORED);
    expand('name is Input · body.supplier');

    fireEvent.change(jsonBox(), { target: { value: '' } });

    expect(
      screen.getByText('Empty: saving the step leaves this input out.')
    ).toBeTruthy();
    // Not an immediate '', which validation rejects for a condition (E025).
    expect(save()).toEqual({ schema_name: STORED.schema_name });
  });

  it('returns to the JSON box after another mode is picked', () => {
    const { save, entry } = openStep(STORED);
    expand('name is Input · body.supplier');

    fireEvent.click(modeButton('Reference'));
    fireEvent.click(modeButton('Value'));

    expect(jsonBox().value).toBe('');
    expect(screen.queryByPlaceholderText('Enter a value...')).toBeNull();
    expect(entry('condition')?.typeHint).toBe('json');

    fireEvent.change(jsonBox(), {
      target: { value: JSON.stringify(CONDITION) },
    });
    expect(save().condition).toEqual({
      valueType: 'immediate',
      value: CONDITION,
    });
  });
});

describe('SimpleInputMappingEditor — plain values on an `any` input', () => {
  const STORED = {
    schema_name: { valueType: 'immediate', value: 'suppliers' },
    order_by: { valueType: 'immediate', value: 'name' },
  };
  const ANY_FIELDS = [
    ...FIELDS,
    { name: 'order_by', type: 'any', required: false },
  ] as unknown as CapabilityField[];

  it('keeps the plain value box after a mode switch', () => {
    const { save, entry } = openStep(STORED, ANY_FIELDS);
    expand('Click to configure...');

    fireEvent.click(modeButton('Reference'));
    fireEvent.click(modeButton('Value'));
    fireEvent.change(screen.getByPlaceholderText('Enter a value...'), {
      target: { value: 'city' },
    });

    expect(screen.queryByPlaceholderText('Enter a JSON value...')).toBeNull();
    expect(entry('order_by')?.typeHint).not.toBe('json');
    expect(save().order_by).toEqual({ valueType: 'immediate', value: 'city' });
  });

  it('still saves an explicitly emptied plain value as an immediate ""', () => {
    // Passing "" to an input is legal DSL; only an emptied JSON literal is
    // treated as unset.
    const { save } = openStep(STORED, ANY_FIELDS);
    expand('Click to configure...');

    fireEvent.change(screen.getByPlaceholderText('Enter a value...'), {
      target: { value: '' },
    });

    expect(save().order_by).toEqual({ valueType: 'immediate', value: '' });
  });
});

describe('SimpleInputMappingEditor — immediate structures on typed inputs', () => {
  it('shows an immediate object on a typed object input as JSON', () => {
    const filters = { supplier: 'acme', active: true };
    const stored = {
      schema_name: { valueType: 'immediate', value: 'suppliers' },
      filters: { valueType: 'immediate', value: filters },
    };
    const { save } = openStep(stored);
    expand('2 fields');

    expect(screen.queryByText('Unknown type')).toBeNull();
    // A typed object input offers Value while it holds a literal.
    expect(modeButton('Value')).toBeTruthy();
    expect(jsonBox().value).toBe(JSON.stringify(filters, null, 2));
    expect(save()).toEqual(stored);
  });

  it('shows an immediate list on an array input instead of an empty Build', () => {
    const stored = {
      schema_name: { valueType: 'immediate', value: 'suppliers' },
      fields: { valueType: 'immediate', value: ['sku', 'name'] },
    };
    const { save } = openStep(stored);
    expand('2 items');

    expect(jsonBox().value).toBe(JSON.stringify(['sku', 'name'], null, 2));
    expect(save()).toEqual(stored);

    fireEvent.change(jsonBox(), { target: { value: '["sku"]' } });
    expect(save().fields).toEqual({ valueType: 'immediate', value: ['sku'] });
  });

  it('keeps Value reachable on a typed object input after Build is picked', () => {
    const stored = {
      schema_name: { valueType: 'immediate', value: 'suppliers' },
      filters: { valueType: 'immediate', value: { supplier: 'acme' } },
    };
    const { save } = openStep(stored);
    expand('1 field');

    fireEvent.click(modeButton('Build'));
    // Value used to disappear here, leaving the literal unrecoverable.
    fireEvent.click(modeButton('Value'));
    fireEvent.change(jsonBox(), {
      target: { value: '{"supplier": "globex"}' },
    });

    expect(save().filters).toEqual({
      valueType: 'immediate',
      value: { supplier: 'globex' },
    });
  });

  it('keeps Value reachable on an array input after Build is picked', () => {
    const stored = {
      schema_name: { valueType: 'immediate', value: 'suppliers' },
      fields: { valueType: 'immediate', value: ['sku'] },
    };
    const { save } = openStep(stored);
    expand('1 item');

    fireEvent.click(modeButton('Build'));
    fireEvent.click(modeButton('Value'));
    fireEvent.change(jsonBox(), { target: { value: '["sku", "city"]' } });

    expect(save().fields).toEqual({
      valueType: 'immediate',
      value: ['sku', 'city'],
    });
  });

  it('makes an empty object input a composite on its first Build edit', () => {
    // A new step seeds object inputs as an empty immediate that opens in
    // Build; adding a field there must still produce a composite.
    const fields = [
      ...FIELDS,
      { name: 'payload', type: 'object', required: true },
    ] as unknown as CapabilityField[];
    const { save } = openStep(
      { schema_name: { valueType: 'immediate', value: 'suppliers' } },
      fields
    );
    expand('Click to configure...');

    fireEvent.click(screen.getByText('Add Field'));
    fireEvent.change(screen.getByPlaceholderText('Enter field name...'), {
      target: { value: 'note' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Add' }));

    expect(save().payload).toEqual({
      valueType: 'composite',
      value: { note: { valueType: 'immediate', value: '' } },
    });
  });

  it('still opens a built object in Build', () => {
    const stored = {
      schema_name: { valueType: 'immediate', value: 'suppliers' },
      filters: {
        valueType: 'composite',
        value: {
          supplier: { valueType: 'reference', value: 'data.body.supplier' },
        },
      },
    };
    const { save } = openStep(stored);

    // A composite opens expanded: its editor is the only place it shows.
    expect(screen.getAllByText(/Composite Object/).length).toBeGreaterThan(0);
    expect(screen.queryByPlaceholderText('Enter a JSON value...')).toBeNull();
    expect(screen.queryByText('Unknown type')).toBeNull();
    expect(save()).toEqual(stored);
  });
});
