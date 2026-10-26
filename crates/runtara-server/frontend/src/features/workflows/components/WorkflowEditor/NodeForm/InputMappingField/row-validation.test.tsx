import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react';
import { zodResolver } from '@hookform/resolvers/zod';
import { FormProvider, useForm, type UseFormReturn } from 'react-hook-form';
import { afterEach, describe, expect, it } from 'vitest';

import { useNodeFormStore } from '@/features/workflows/stores/nodeFormStore';
import { NodeFormContext } from '../NodeFormContext';
import { initialValues, schema } from '../NodeFormItem';
import { InputMappingField } from './index';

/**
 * The step panel writes its rows to the form with setValue on the
 * `inputMapping` field array, which react-hook-form does not validate, so a
 * row's "Invalid JSON" used to stay after the JSON was fixed, until the next
 * submit. These tests render the field with the node form's own resolver.
 */

const NODE_ID = 'step-1';

const CONDITION = {
  type: 'operation',
  op: 'EQ',
  arguments: [
    { valueType: 'reference', value: 'name' },
    { valueType: 'reference', value: 'data.body.supplier' },
  ],
};

const agents = [
  {
    id: 'object-model',
    name: 'Object Model',
    supportedCapabilities: {
      'query-instances': {
        id: 'query-instances',
        inputs: [
          { name: 'schema_name', type: 'string', required: true },
          { name: 'condition', type: 'any', required: false },
        ],
      },
    },
  },
];

afterEach(() => useNodeFormStore.getState().clearNode(NODE_ID));

function renderField() {
  let form: UseFormReturn | undefined;
  function Harness() {
    // The node form's configuration (NodeForm/index.tsx).
    const methods = useForm({
      resolver: zodResolver(schema()),
      mode: 'onChange',
      defaultValues: {
        ...initialValues,
        stepType: 'Agent',
        agentId: 'object-model',
        capabilityId: 'query-instances',
        inputMapping: [
          { type: 'schema_name', value: 'suppliers', valueType: 'immediate' },
          { type: 'condition', value: CONDITION, valueType: 'immediate' },
        ],
      },
    });
    form = methods as unknown as UseFormReturn;
    return (
      <FormProvider {...methods}>
        <InputMappingField label="Inputs" name="inputMapping" />
      </FormProvider>
    );
  }
  const ctx = {
    previousSteps: [],
    inputSchemaFields: [],
    variables: [],
    isInsideSplit: false,
    isInsideWaitScope: false,
    splitItemSchemaFields: [],
    nodeId: NODE_ID,
    agents,
    workflows: [],
    stepTypes: [],
  };
  render(
    <NodeFormContext.Provider value={ctx as never}>
      <Harness />
    </NodeFormContext.Provider>
  );

  fireEvent.click(screen.getByText('name is Input · body.supplier'));
  const type = (text: string) =>
    fireEvent.change(screen.getByPlaceholderText('Enter a JSON value...'), {
      target: { value: text },
    });
  const submit = () =>
    act(async () => {
      await form!.handleSubmit(() => {})();
    });
  return { type, submit };
}

const jsonError = () => screen.queryByText(/^Invalid JSON — /);

describe('InputMappingField — row errors', () => {
  it('does not flag half-typed JSON before the step is first applied', async () => {
    const { type } = renderField();

    type('{"type":');
    await new Promise((r) => setTimeout(r, 50));

    expect(jsonError()).toBeNull();
  });

  it('clears a fixed JSON error without another apply', async () => {
    const { type, submit } = renderField();

    type('{"type":');
    await submit();
    expect(jsonError()).not.toBeNull();

    type(JSON.stringify(CONDITION));
    await waitFor(() => expect(jsonError()).toBeNull());
  });

  it('keeps the error current while the JSON is edited after an apply', async () => {
    const { type, submit } = renderField();

    // Applied with valid input: nothing to show yet.
    await submit();
    expect(jsonError()).toBeNull();

    type('{"type":');
    await waitFor(() => expect(jsonError()).not.toBeNull());

    type(JSON.stringify(CONDITION));
    await waitFor(() => expect(jsonError()).toBeNull());
  });
});
