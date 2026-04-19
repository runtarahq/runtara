import { useContext, useMemo, useEffect, useState } from 'react';
import { useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { Loader2, RotateCcw, Trash2 } from 'lucide-react';
import { NextForm } from '@/shared/components/NextForm';
import { SheetBase } from '@/shared/components/next-dialog/sheet-base.tsx';
import { FormContent } from '@/shared/components/NextForm/form-content.tsx';
import { Button } from '@/shared/components/ui/button';
import { NodeFormContext } from './NodeFormContext.tsx';
import * as form from './NodeFormItem.tsx';
import { TabProvider, useTabContext } from './NodeFormItem.tsx';
import { getTestHandler } from './TestAgentButton/TestAgentInline';

type Props = {
  isEdit?: boolean;
  values: form.SchemaType;
  originalValues?: form.SchemaType;
  onSubmit: (data: form.SchemaType) => void;
  onChange?: (data: form.SchemaType) => void;
  onReset?: () => void;
  onDelete?: () => void;
};

export function NodeForm({
  isEdit,
  values,
  originalValues,
  onSubmit,
  onChange,
  onReset,
  onDelete,
}: Props) {
  const { stepTypes, agents, isLoading } = useContext(NodeFormContext);

  const formProps = useMemo(() => ({ stepTypes, agents }), [stepTypes, agents]);

  const entireForm = useForm<form.SchemaType>({
    resolver: zodResolver(form.schema(formProps)),
    defaultValues: (values || form.initialValues) as form.SchemaType,
    mode: 'onChange',
  });

  // Reset form values when values prop changes from external source
  // Skip reset if the incoming values match current form values (prevents focus loss)
  useEffect(() => {
    // Deep clone values to avoid readonly/frozen object errors
    const clonedValues = values ? JSON.parse(JSON.stringify(values)) : {};
    const mergedValues = {
      ...form.initialValues,
      ...clonedValues,
    };

    // Compare incoming values with current form values
    // If they're the same, skip reset to preserve focus
    const currentValues = entireForm.getValues();
    const currentJson = JSON.stringify(currentValues);
    const incomingJson = JSON.stringify(mergedValues);

    if (currentJson === incomingJson) {
      // Values are the same, no need to reset
      return;
    }

    entireForm.reset(mergedValues);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [values]);

  // Watch for form changes and notify parent immediately (for staging in parent dialog)
  useEffect(() => {
    if (!onChange) return;

    const subscription = entireForm.watch((formValues, { name: fieldName }) => {
      // Only call onChange for user input, not for reset/setValue calls
      // fieldName will be undefined for resets, and defined for user changes
      if (!fieldName) {
        return;
      }

      onChange(formValues as form.SchemaType);
    });

    return () => {
      subscription.unsubscribe();
    };
  }, [entireForm, onChange]);

  const handleSubmit = (data: form.SchemaType) => {
    // console.log("[DEBUG] NodeForm handleSubmit - data.inputMapping:', data.inputMapping);
    onSubmit(data);
  };

  const handleReset = () => {
    if (originalValues) {
      const clonedOriginal = JSON.parse(JSON.stringify(originalValues));
      const mergedValues = {
        ...form.initialValues,
        ...clonedOriginal,
      };
      entireForm.reset(mergedValues);
      onReset?.();
    }
  };

  const stepType = entireForm.watch('stepType');

  // Filter out fields that should be hidden for Start steps
  const filteredFieldsConfig =
    stepType === 'Start'
      ? form.fieldsConfig.filter((field: any) => {
          // Keep these fields for Start steps
          if (field.name === 'stepType') return true;
          if (field.name === 'name') return true;
          if (field.name === 'inputMapping') return true;
          if (field.name === 'childWorkflowId') return true;
          if (field.name === 'childVersion') return true;
          if (field.name === 'inputSchema') return true;
          if (field.name === 'inputSchemaFields') return true;
          if (field.name === 'startMode') return true;
          if (field.name === 'selectedTriggerId') return true;

          // Hide these fields for Start steps
          if (field.name === 'agentId') return false;
          if (field.name === 'capabilityId') return false;
          if (field.name === 'connectionId') return false;
          if (field.name === 'embedWorkflowConfig') return false;
          if (field.name === 'formTabs') return false;

          return true;
        })
      : form.fieldsConfig;

  const renderContent = () => (
    <FormContentWrapper
      isLoading={isLoading}
      fieldsConfig={filteredFieldsConfig}
    />
  );

  const renderActions = () => (
    <FormActions isEdit={isEdit} onReset={handleReset} onDelete={onDelete} />
  );

  return (
    <TabProvider>
      <NextForm
        className="flex flex-col h-full"
        form={entireForm}
        formProps={formProps}
        renderContent={renderContent}
        renderActions={renderActions}
        onSubmit={(data) => {
          handleSubmit(data || entireForm.getValues());
        }}
      />
    </TabProvider>
  );
}

function FormActions({
  isEdit,
  onReset,
  onDelete,
}: {
  isEdit?: boolean;
  onReset: () => void;
  onDelete?: () => void;
}) {
  const { activeTab } = useTabContext();
  const [, forceUpdate] = useState(0);

  // Force re-render periodically to get latest test handler state
  useEffect(() => {
    if (activeTab === 'testing') {
      const interval = setInterval(() => forceUpdate((n) => n + 1), 100);
      return () => clearInterval(interval);
    }
  }, [activeTab]);

  // Main tab: Show Reset button (for edit mode) or Save button (for create mode)
  if (activeTab === 'main') {
    if (isEdit) {
      return (
        <div className="flex justify-between pt-4 mt-auto border-t">
          <Button
            type="button"
            variant="outline"
            className="px-6 text-destructive hover:text-destructive hover:bg-destructive/10"
            onClick={onDelete}
          >
            <Trash2 className="mr-2 h-4 w-4" />
            Delete
          </Button>
          <Button
            type="button"
            variant="outline"
            className="px-6"
            onClick={onReset}
          >
            <RotateCcw className="mr-2 h-4 w-4" />
            Reset
          </Button>
        </div>
      );
    }
    // Create mode - show Save button
    return (
      <div className="flex justify-end pt-4 mt-auto border-t">
        <Button type="submit" className="px-6">
          Save
        </Button>
      </div>
    );
  }

  // Testing tab: Show Run Test button
  if (activeTab === 'testing') {
    const testHandler = getTestHandler();

    return (
      <div className="flex justify-end pt-4 mt-auto border-t">
        <Button
          type="button"
          className="px-6"
          onClick={() => testHandler?.runTest()}
          disabled={
            !testHandler?.isAvailable ||
            !testHandler?.isValid ||
            testHandler?.isPending
          }
        >
          {testHandler?.isPending && (
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
          )}
          Run Test
        </Button>
      </div>
    );
  }

  return null;
}

function FormContentWrapper({
  isLoading,
  fieldsConfig,
}: {
  isLoading: boolean;
  fieldsConfig: any[];
}) {
  return (
    <div className="flex flex-col h-full min-h-0">
      <SheetBase loading={isLoading}>
        <FormContent fieldsConfig={fieldsConfig} />
      </SheetBase>
    </div>
  );
}
