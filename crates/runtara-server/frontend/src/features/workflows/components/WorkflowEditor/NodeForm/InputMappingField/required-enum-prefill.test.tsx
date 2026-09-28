import { render, waitFor } from '@testing-library/react';
import {
  FormProvider,
  useForm,
  type FieldValues,
  type UseFormReturn,
} from 'react-hook-form';
import { describe, expect, it } from 'vitest';

import { InputMappingField } from './index';
import {
  applyRequiredEnumPrefills,
  getRequiredEnumPrefill,
  type EnumPrefillField,
} from './required-enum-prefill';
import { NodeFormContext } from '../NodeFormContext';

/**
 * D3: a required string-enum input with no default and no mapping (the
 * control agent's `start.parentClosePolicy`) is pre-filled with enum[0] so
 * the step saves a value instead of tripping E022.
 */
const parentClosePolicy: EnumPrefillField = {
  name: 'parentClosePolicy',
  type: 'string',
  required: true,
  enum: ['cancel', 'leave_running'],
};

const typeHint = () => 'string';

describe('getRequiredEnumPrefill', () => {
  it('returns enum[0] for a required enum without a default', () => {
    expect(getRequiredEnumPrefill(parentClosePolicy)).toBe('cancel');
  });

  it('ignores optional enums', () => {
    expect(
      getRequiredEnumPrefill({ ...parentClosePolicy, required: false })
    ).toBeUndefined();
  });

  it('ignores enums with a default', () => {
    expect(
      getRequiredEnumPrefill({ ...parentClosePolicy, default: 'leave_running' })
    ).toBeUndefined();
  });

  it('ignores non-enum fields', () => {
    expect(
      getRequiredEnumPrefill({ name: 'x', type: 'string', required: true })
    ).toBeUndefined();
  });
});

describe('applyRequiredEnumPrefills', () => {
  it('pre-fills a required enum with no mapping as an immediate value', () => {
    const result = applyRequiredEnumPrefills([parentClosePolicy], [], typeHint);
    expect(result?.prefilled).toEqual(['parentClosePolicy']);
    expect(result?.entries).toEqual([
      {
        type: 'parentClosePolicy',
        value: 'cancel',
        valueType: 'immediate',
        typeHint: 'string',
      },
    ]);
  });

  it('replaces the untouched empty auto-seeded row', () => {
    const result = applyRequiredEnumPrefills(
      [parentClosePolicy],
      [
        {
          type: 'parentClosePolicy',
          value: '',
          valueType: 'immediate',
          autoSeeded: true,
        },
      ],
      typeHint
    );
    expect(result?.entries).toEqual([
      {
        type: 'parentClosePolicy',
        value: 'cancel',
        valueType: 'immediate',
        typeHint: 'string',
      },
    ]);
  });

  it('does not pre-fill optional enums', () => {
    expect(
      applyRequiredEnumPrefills(
        [{ ...parentClosePolicy, required: false }],
        [],
        typeHint
      )
    ).toBeNull();
  });

  it('leaves an existing immediate mapping unchanged', () => {
    expect(
      applyRequiredEnumPrefills(
        [parentClosePolicy],
        [
          {
            type: 'parentClosePolicy',
            value: 'leave_running',
            valueType: 'immediate',
          },
        ],
        typeHint
      )
    ).toBeNull();
  });

  it('leaves an existing reference mapping unchanged', () => {
    expect(
      applyRequiredEnumPrefills(
        [parentClosePolicy],
        [
          {
            type: 'parentClosePolicy',
            value: 'data.policy',
            valueType: 'reference',
          },
        ],
        typeHint
      )
    ).toBeNull();
  });

  it('leaves an explicit empty value (not auto-seeded) unchanged', () => {
    expect(
      applyRequiredEnumPrefills(
        [parentClosePolicy],
        [{ type: 'parentClosePolicy', value: '', valueType: 'immediate' }],
        typeHint
      )
    ).toBeNull();
  });

  it('does not touch enums with a default (existing default handling)', () => {
    expect(
      applyRequiredEnumPrefills(
        [{ ...parentClosePolicy, default: 'leave_running' }],
        [],
        typeHint
      )
    ).toBeNull();
  });

  it('never re-fills a field already pre-filled once', () => {
    expect(
      applyRequiredEnumPrefills(
        [parentClosePolicy],
        [],
        typeHint,
        new Set(['parentClosePolicy'])
      )
    ).toBeNull();
  });
});

describe('InputMappingField — required enum pre-fill', () => {
  const agents = [
    {
      id: 'control',
      name: 'Control',
      supportedCapabilities: {
        start: {
          id: 'start',
          inputs: [
            parentClosePolicy,
            {
              name: 'mode',
              type: 'string',
              required: false,
              enum: ['a', 'b'],
            },
          ],
        },
      },
    },
  ];

  function renderField(inputMapping: unknown[]) {
    let form: UseFormReturn | undefined;
    function Harness() {
      const methods = useForm<FieldValues>({
        defaultValues: {
          stepType: 'Agent',
          agentId: 'control',
          capabilityId: 'start',
          inputMapping,
        },
      });
      form = methods;
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
      nodeId: 'step-1',
      agents,
      workflows: [],
      stepTypes: [],
    };
    render(
      <NodeFormContext.Provider value={ctx as never}>
        <Harness />
      </NodeFormContext.Provider>
    );
    return () => form!.getValues('inputMapping');
  }

  it('writes enum[0] into the form mapping for an unmapped required enum', async () => {
    const getMapping = renderField([]);
    await waitFor(() =>
      expect(getMapping()).toContainEqual(
        expect.objectContaining({
          type: 'parentClosePolicy',
          value: 'cancel',
          valueType: 'immediate',
        })
      )
    );
    // The optional enum is not added.
    expect(getMapping().some((e: { type: string }) => e.type === 'mode')).toBe(
      false
    );
  });

  it('keeps a user-chosen reference mapping', async () => {
    const existing = {
      type: 'parentClosePolicy',
      value: 'data.policy',
      valueType: 'reference',
      typeHint: 'string',
    };
    const getMapping = renderField([existing]);
    // Let effects settle, then assert nothing overwrote the reference.
    await waitFor(() => expect(getMapping()).toBeDefined());
    await new Promise((r) => setTimeout(r, 80));
    expect(
      getMapping().filter(
        (e: { type: string }) => e.type === 'parentClosePolicy'
      )
    ).toEqual([
      expect.objectContaining({ value: 'data.policy', valueType: 'reference' }),
    ]);
  });
});
