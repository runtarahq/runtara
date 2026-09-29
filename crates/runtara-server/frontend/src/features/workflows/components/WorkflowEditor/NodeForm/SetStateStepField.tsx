import { useContext } from 'react';
import { useFormContext, useWatch } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormItem,
  FormLabel,
} from '@/shared/components/ui/form';
import { Button } from '@/shared/components/ui/button';
import { NodeFormContext } from './NodeFormContext';
import { SchemaField } from '../EditorSidebar/SchemaFieldsEditor';
import {
  MappingValueInput,
  ValueMode,
} from './InputMappingField/MappingValueInput';

type SetStateStepFieldProps = {
  name: string;
};

/** A declared state field, as the workflow's `stateSchema` has it. */
type StateField = {
  type?: string;
  label?: string;
  description?: string;
  format?: string;
  enum?: unknown[];
};

/**
 * The workflow's declared state fields: the editor's (possibly staged)
 * fields when it has them, otherwise the saved graph's `stateSchema`.
 */
function declaredStateFields(
  stateSchemaFields: SchemaField[] | undefined,
  executionGraph: unknown
): Record<string, StateField> {
  if (stateSchemaFields) {
    return Object.fromEntries(
      stateSchemaFields
        .filter((field) => field.name)
        .map((field) => [field.name, field])
    );
  }
  return (
    (executionGraph as { stateSchema?: Record<string, StateField> } | null)
      ?.stateSchema ?? {}
  );
}

/** Mapping rows of a SetState step: one per state field it writes. */
type Row = { type: string; value: unknown; valueType?: ValueMode };

const VALUE_MODES: readonly ValueMode[] = [
  'immediate',
  'reference',
  'template',
];

/**
 * The values a SetState step writes, one per state field declared in the
 * workflow's `stateSchema`. A field is written only once it has a row; a
 * field left untouched keeps its current state value.
 */
export function SetStateStepField({ name }: SetStateStepFieldProps) {
  const form = useFormContext();
  const { executionGraph, stateSchemaFields } = useContext(NodeFormContext);
  const stepType = useWatch({ name: 'stepType', control: form.control });
  const rows: Row[] = useWatch({
    name,
    control: form.control,
    defaultValue: [],
  });

  if (stepType !== 'SetState') {
    return null;
  }

  const stateSchema = declaredStateFields(stateSchemaFields, executionGraph);
  const fields = Object.entries(stateSchema).sort(([a], [b]) =>
    a.localeCompare(b)
  );
  const options = {
    shouldDirty: true,
    shouldTouch: true,
    shouldValidate: true,
  };
  const rowOf = (field: string) =>
    (rows || []).findIndex((row) => row.type === field);

  const update = (field: string, value: unknown, valueType?: ValueMode) => {
    const mapping: Row[] = form.getValues(name) || [];
    const index = mapping.findIndex((row) => row.type === field);
    if (index >= 0) {
      form.setValue(`${name}.${index}.value`, value, options);
      if (valueType !== undefined) {
        form.setValue(`${name}.${index}.valueType`, valueType, options);
      }
      return;
    }
    form.setValue(
      name,
      [
        ...mapping,
        {
          type: field,
          value,
          typeHint: stateSchema[field]?.type ?? 'string',
          valueType: valueType || 'immediate',
        },
      ],
      options
    );
  };

  const remove = (field: string) => {
    const mapping: Row[] = form.getValues(name) || [];
    form.setValue(
      name,
      mapping.filter((row) => row.type !== field),
      options
    );
  };

  return (
    <div className="space-y-4">
      <div>
        <p className="text-sm font-medium">Set State</p>
        <p className="text-xs text-muted-foreground">
          Merges these values into the run&apos;s state. Other workflows read it
          with the control agent&apos;s get-state and query, without waking the
          run. A field left out keeps its value.
        </p>
      </div>

      {fields.length === 0 && (
        <p className="text-sm text-muted-foreground">
          Declare the workflow&apos;s state in Settings › State first.
        </p>
      )}

      {fields.map(([field, schema]) => {
        const index = rowOf(field);
        const row = index >= 0 ? rows[index] : undefined;
        const valueType =
          (row?.valueType as ValueMode | undefined) ?? 'immediate';
        return (
          <FormItem key={field}>
            <div className="flex items-center justify-between">
              <FormLabel>{schema.label || field}</FormLabel>
              {row && (
                <Button
                  type="button"
                  variant="secondary"
                  size="sm"
                  onClick={() => remove(field)}
                >
                  Don&apos;t write
                </Button>
              )}
            </div>
            <FormDescription>
              <code>{field}</code>
              {schema.type ? ` · ${schema.type}` : ''}
              {schema.format ? ` · ${schema.format}` : ''}
              {schema.enum?.length
                ? ` · one of ${schema.enum.map(String).join(', ')}`
                : ''}
              {row ? '' : ' · not written'}
            </FormDescription>
            <FormControl>
              <MappingValueInput
                value={row ? String(row.value ?? '') : ''}
                onChange={(value) => update(field, value ?? '')}
                valueType={valueType}
                onValueTypeChange={(vt) => update(field, row?.value ?? '', vt)}
                modes={VALUE_MODES}
                fieldType={schema.type ?? 'string'}
                fieldName={field}
                placeholder={schema.description ?? ''}
              />
            </FormControl>
          </FormItem>
        );
      })}
    </div>
  );
}
