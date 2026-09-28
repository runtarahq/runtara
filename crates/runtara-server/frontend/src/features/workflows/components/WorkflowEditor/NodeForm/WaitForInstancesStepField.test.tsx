import { useEffect, useState } from 'react';
import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { FormProvider, useForm, type UseFormReturn } from 'react-hook-form';
import { NodeFormContext, NodeFormContextContextData } from './NodeFormContext';
import { WaitForInstancesStepField } from './WaitForInstancesStepField';
import { canStepHaveErrorHandler } from '@/features/workflows/utils/step-error-support';

type FormValues = Record<string, unknown>;

function renderField(values: FormValues, nodeId?: string) {
  let formRef: UseFormReturn<FormValues> | null = null;
  function Harness() {
    const form = useForm<FormValues>({
      defaultValues: { stepType: 'WaitForInstances', ...values },
    });
    formRef = form;
    // Mount the field once the form is mounted, as in the editor, so
    // useWatch reads the loaded values rather than its own default.
    const [mounted, setMounted] = useState(false);
    useEffect(() => setMounted(true), []);
    if (!mounted) return null;
    return (
      <NodeFormContext.Provider
        value={
          {
            stepTypes: [],
            agents: [],
            workflows: [],
            executionGraph: null,
            isLoading: false,
            previousSteps: [],
            nodeId,
          } as NodeFormContextContextData
        }
      >
        <FormProvider {...form}>
          <WaitForInstancesStepField name="inputMapping" />
        </FormProvider>
      </NodeFormContext.Provider>
    );
  }
  const utils = render(<Harness />);
  return { ...utils, getForm: () => formRef! };
}

describe('WaitForInstancesStepField', () => {
  it('seeds a new step with instanceIds, mode all and an empty timeout', () => {
    const { getForm } = renderField({ inputMapping: [] });

    expect(getForm().getValues('inputMapping')).toEqual([
      {
        type: 'instanceIds',
        value: '',
        typeHint: 'array',
        valueType: 'reference',
      },
      {
        type: 'mode',
        value: 'all',
        typeHint: 'string',
        valueType: 'immediate',
      },
      {
        type: 'timeoutMs',
        value: '',
        typeHint: 'number',
        valueType: 'immediate',
      },
    ]);
    expect(
      screen.getByText('Wait for Instances Configuration')
    ).toBeInTheDocument();
    expect(screen.getByText('Instance IDs *')).toBeInTheDocument();
    expect(screen.getByText('Timeout (ms)')).toBeInTheDocument();
  });

  it('shows a loaded step without resetting its mapping', () => {
    const loaded = [
      {
        type: 'instanceIds',
        value: '["run-a"]',
        typeHint: 'array',
        valueType: 'immediate',
      },
      {
        type: 'mode',
        value: 'any',
        typeHint: 'string',
        valueType: 'immediate',
      },
      {
        type: 'timeoutMs',
        value: 5000,
        typeHint: 'number',
        valueType: 'immediate',
      },
    ];
    const { getForm } = renderField({ inputMapping: loaded }, 'waitRuns');

    expect(getForm().getValues('inputMapping')).toEqual(loaded);
    expect(screen.getByDisplayValue('["run-a"]')).toBeInTheDocument();
    expect(screen.getByDisplayValue('5000')).toBeInTheDocument();
    expect(screen.getByRole('combobox', { name: 'Mode' })).toHaveTextContent(
      'Any'
    );
  });

  it('edits a composite list of start outputs in the composite editor', () => {
    renderField(
      {
        inputMapping: [
          {
            type: 'instanceIds',
            value: [
              {
                valueType: 'reference',
                value: 'steps.startA.outputs.instanceId',
              },
              {
                valueType: 'reference',
                value: 'steps.startB.outputs.instanceId',
              },
            ],
            typeHint: 'array',
            valueType: 'composite',
          },
        ],
      },
      'waitRuns'
    );

    expect(
      screen.getByText('Composite array - configure below')
    ).toBeInTheDocument();
    expect(
      screen.getByText(/steps\.startA\.outputs\.instanceId/)
    ).toBeInTheDocument();
  });

  it('renders nothing for another step type', () => {
    const { container } = renderField({
      stepType: 'WaitForSignal',
      inputMapping: [],
    });
    expect(container).toBeEmptyDOMElement();
  });

  it('offers the error route under both spellings of the step type', () => {
    expect(canStepHaveErrorHandler('WaitForInstances')).toBe(true);
    expect(canStepHaveErrorHandler('Wait for Instances')).toBe(true);
  });
});
