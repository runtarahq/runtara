import { useController, useFieldArray, useFormContext } from 'react-hook-form';
import { Icons } from '@/shared/components/icons.tsx';
import {
  ConditionEditor,
  type Condition,
} from '@/shared/components/ui/condition-editor';
import { ErrorConditionTemplates } from '@/shared/components/ErrorConditionTemplates';
import { useContext, useEffect, useMemo, useRef } from 'react';
import { NodeFormContext } from '../NodeFormContext';
import { SwitchCasesField } from '../SwitchCasesField';
import { SimpleInputMappingEditor } from './SimpleInputMappingEditor';
import { ValueType } from '../TypeHintSelector';
import { parseSchema } from '@/features/scenarios/utils/schema';

interface ParsedField {
  name: string;
  type: string;
  description?: string;
  required: boolean;
  defaultValue?: any;
  enum?: string[];
}

/**
 * Maps JSON schema type to ValueType
 */
function getValueTypeFromSchemaType(schemaType: string): ValueType {
  const lowerType = schemaType.toLowerCase();

  if (lowerType === 'string') return ValueType.String;
  if (lowerType === 'boolean' || lowerType === 'bool') return ValueType.Boolean;
  if (lowerType === 'integer' || lowerType === 'int') return ValueType.Integer;
  if (lowerType === 'number' || lowerType === 'float' || lowerType === 'double')
    return ValueType.Number;
  if (
    lowerType === 'array' ||
    lowerType.startsWith('[') ||
    lowerType.includes('array<')
  )
    return ValueType.Json;
  if (lowerType === 'object' || lowerType.startsWith('{'))
    return ValueType.Json;
  if (lowerType === 'file') return ValueType.File;

  // Default to string for unknown types
  return ValueType.String;
}

export function InputMappingField(props: any) {
  const { label, name } = props;
  const { watch, setValue } = useFormContext();
  const { agents, scenarios, previousSteps, nodeId } =
    useContext(NodeFormContext);
  // We're in edit mode only if we have a nodeId (not parentNodeId)
  // parentNodeId means we're creating a child node, not editing
  const isEdit = !!nodeId;

  // Watch the connectionId to filter out connection_id from input mapping
  const connectionId = watch('connectionId');

  const {
    fieldState: { error },
  } = useController({ name });

  const { append, remove } = useFieldArray({ name });

  // Use watch from context to get the current form value
  // useWatch with defaultValue: [] can return empty array before form is ready
  const watchFieldArray = watch(name) ?? [];
  const stepType = watch('stepType');
  const capabilityId = watch('capabilityId'); // Get the capabilityId field for conditional nodes
  // Also watch the 'condition' field - backend stores condition there for Conditional steps
  const conditionField = watch('condition');
  const agentId = watch('agentId');
  const childScenarioId = watch('childScenarioId'); // Get childScenarioId for StartScenario steps
  const isConditional = stepType === 'Conditional';
  const isSwitch = stepType === 'Switch';
  const isStartScenario = stepType === 'StartScenario';

  // Ensure watchFieldArray is an array
  const fieldArray = Array.isArray(watchFieldArray) ? watchFieldArray : [];

  // Track if we've initialized the default condition for new Conditional steps
  const hasInitializedConditionRef = useRef(false);

  // Initialize default condition when Conditional step is first created (no existing condition)
  // This ensures the condition is saved to the form even if user doesn't modify it
  useEffect(() => {
    if (
      isConditional &&
      !conditionField &&
      !hasInitializedConditionRef.current
    ) {
      hasInitializedConditionRef.current = true;
      const defaultCondition = {
        type: 'operation',
        op: 'EQ',
        arguments: [
          { valueType: 'immediate', value: '', immediateType: 'string' },
          { valueType: 'immediate', value: '', immediateType: 'string' },
        ],
      };
      // Save the default condition to form state
      setValue('condition', defaultCondition);
    }
  }, [isConditional, conditionField, setValue]);

  // Get input fields from child scenario's inputSchema for StartScenario steps
  // Must be called unconditionally to satisfy React hooks rules
  const childScenarioInputFields = useMemo(() => {
    if (!isStartScenario || !childScenarioId || !scenarios) {
      return [];
    }

    // Find the child scenario
    const childScenario = scenarios.find((s) => s.id === childScenarioId);
    if (!childScenario?.inputSchema) {
      return [];
    }

    // Parse the inputSchema to get field definitions
    const schemaFields = parseSchema(childScenario.inputSchema);

    // Convert to the CapabilityField-like format expected by SimpleInputMappingEditor
    return schemaFields.map((field) => ({
      name: field.name,
      type: field.type || 'string',
      description: field.description,
      required: field.required ?? false,
      default: field.defaultValue,
    }));
  }, [isStartScenario, childScenarioId, scenarios]);

  // Track if we've already auto-populated for this capability to prevent duplicates
  const autoPopulatedRef = useRef<string | null>(null);

  // Auto-populate fields when capability changes (for new nodes only)
  useEffect(() => {
    // Skip if no capability selected
    if (!capabilityId) {
      return;
    }

    // For non-Agent steps, we don't need agent schema
    if (stepType !== 'Agent') {
      return;
    }

    if (!agentId || !agents) {
      return;
    }

    // Get schema from agent directly (no API call needed)
    const agent = agents.find((a) => a.id === agentId);
    const capability = agent?.supportedCapabilities?.[capabilityId];

    // Use the inputs array from the capability
    if (capability?.inputs && Array.isArray(capability.inputs)) {
      const parsedFields: ParsedField[] = capability.inputs
        .filter((field) => {
          // Filter out CONNECTION_DATA fields
          if (field.name.startsWith('get__CONNECTION_DATA')) {
            return false;
          }
          // Filter out connection_id if a connection is already selected
          // The connection is provided at the agent configuration level
          if (
            field.name === 'connection_id' &&
            connectionId &&
            connectionId !== '__none__'
          ) {
            return false;
          }
          return true;
        })
        .map((field) => ({
          name: field.name,
          type: field.type || 'any',
          description: field.description || undefined,
          required: field.required,
          defaultValue: field.default,
        }));

      // Check if the existing fields match the schema fields (to avoid duplicates)
      const existingFieldNames = new Set(
        fieldArray.map((field: any) => field.type).filter(Boolean)
      );
      const schemaFieldNames = new Set(parsedFields.map((field) => field.name));
      const fieldsMatch =
        existingFieldNames.size === schemaFieldNames.size &&
        [...existingFieldNames].every((name) => schemaFieldNames.has(name));

      // Only auto-populate if:
      // 1. We're creating a new node (not editing)
      // 2. AND either there are no existing fields OR the existing fields don't match the schema
      // 3. AND we haven't already auto-populated for this capability
      const capabilityKey = `${agentId}:${capabilityId}`;
      const hasAlreadyAutoPopulated =
        autoPopulatedRef.current === capabilityKey;
      const shouldAutoPopulate =
        parsedFields.length > 0 &&
        !isEdit &&
        !hasAlreadyAutoPopulated &&
        (fieldArray.length === 0 || !fieldsMatch);

      if (shouldAutoPopulate) {
        // Include all fields - objects can be used with JSON type hint
        const newFields = parsedFields.map((field) => ({
          type: field.name,
          value:
            field.defaultValue ??
            (field.enum && field.enum.length > 0 ? field.enum[0] : ''),
          typeHint: getValueTypeFromSchemaType(field.type),
          valueType: 'immediate' as const,
        }));

        if (newFields.length > 0) {
          // Use append() instead of setValue() to properly work with useFieldArray
          // This prevents the "Cannot assign to read only property" error
          append(newFields);
          autoPopulatedRef.current = capabilityKey; // Mark this capability as auto-populated
        }
      }

      // Reset the ref if the capability changes to a different one
      if (
        capabilityKey !== autoPopulatedRef.current &&
        autoPopulatedRef.current !== null
      ) {
        autoPopulatedRef.current = null;
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [capabilityId, agentId, stepType, agents, name, setValue, connectionId]);

  // Hide input mapping when Agent step type is selected but no capability is chosen yet
  // This allows the user to focus on selecting agent and capability first
  if (stepType === 'Agent' && !capabilityId) {
    return null;
  }

  // Handle condition editor value change for Conditional step type
  const handleConditionChange = (value: string) => {
    try {
      const condition = JSON.parse(value);

      // For conditional nodes, save the condition to the 'condition' field (backend expects this)
      setValue('condition', condition);

      // Clear existing fields
      remove();

      // Helper to check if an object is a ConditionArgument
      const isConditionArgument = (
        obj: any
      ): obj is { valueType: 'immediate' | 'reference'; value: string } => {
        return (
          typeof obj === 'object' &&
          obj !== null &&
          'valueType' in obj &&
          'value' in obj &&
          !('op' in obj)
        );
      };

      // Convert the condition to flattened key-value pairs
      // Backend requires 'condition.expression.*' prefix
      // Returns array of { type, value, valueType } objects
      const flattenCondition = (
        obj: any,
        prefix = ''
      ): Array<{
        type: string;
        value: any;
        valueType: 'immediate' | 'reference';
      }> => {
        const result: Array<{
          type: string;
          value: any;
          valueType: 'immediate' | 'reference';
        }> = [];

        for (const key in obj) {
          const newKey = prefix
            ? `${prefix}.${key}`
            : `condition.expression.${key}`;

          if (Array.isArray(obj[key])) {
            // Handle arrays by creating separate entries for each element
            obj[key].forEach((item: any, index: number) => {
              const arrayKey = `${newKey}[${index}]`;
              if (isConditionArgument(item)) {
                // Handle ConditionArgument - preserve valueType
                result.push({
                  type: arrayKey,
                  value: item.value,
                  valueType: item.valueType,
                });
              } else if (typeof item === 'object' && item !== null) {
                result.push(...flattenCondition(item, arrayKey));
              } else {
                // Keep numbers and booleans as their original types
                result.push({
                  type: arrayKey,
                  value: item,
                  valueType: 'immediate',
                });
              }
            });
          } else if (typeof obj[key] === 'object' && obj[key] !== null) {
            result.push(...flattenCondition(obj[key], newKey));
          } else {
            // Keep the original type (number, boolean, string)
            result.push({
              type: newKey,
              value: obj[key],
              valueType: 'immediate',
            });
          }
        }

        return result;
      };

      const flattenedCondition = flattenCondition(condition);

      // Add flattened key-value pairs to the input mapping
      const newFields = flattenedCondition.map(
        ({ type, value, valueType }) => ({
          type,
          value,
          typeHint: 'auto',
          valueType,
        })
      );

      append(newFields);
    } catch (e) {
      console.error('Failed to parse condition:', e);
    }
  };

  // If this is a Conditional step, render the ConditionEditor
  if (isConditional) {
    // Conditional steps store their expression in the 'condition' field
    let conditionValue;

    // Check the 'condition' field
    if (conditionField) {
      if (
        typeof conditionField === 'object' &&
        conditionField !== null &&
        'op' in conditionField
      ) {
        conditionValue = JSON.stringify(conditionField);
      }
    }

    // If no condition, use a default (matching the one set in useEffect above)
    if (!conditionValue) {
      conditionValue = JSON.stringify({
        type: 'operation',
        op: 'EQ',
        arguments: [
          { valueType: 'immediate', value: '', immediateType: 'string' },
          { valueType: 'immediate', value: '', immediateType: 'string' },
        ],
      });
    }

    return (
      <div>
        <div className="mb-3">
          <ErrorConditionTemplates
            onSelect={(condition: Condition) => {
              const conditionStr = JSON.stringify(condition);
              handleConditionChange(conditionStr);
            }}
          />
        </div>
        <ConditionEditor
          value={conditionValue}
          onChange={handleConditionChange}
          previousSteps={previousSteps}
        />
        {error && (
          <div className="text-[0.8rem] mt-2 font-medium text-destructive">
            {error.message || error.root?.message}
          </div>
        )}
      </div>
    );
  }

  // If this is a Switch step, render specialized Switch UI
  if (isSwitch) {
    return (
      <div>
        <SwitchCasesField label={label} name={name} />
      </div>
    );
  }

  // Sync Zustand store changes back to react-hook-form
  const handleSimpleEditorDataChange = (entries: any[]) => {
    // Convert entries to the format expected by react-hook-form
    // Include typeHint to ensure proper type conversion when saving
    const formEntries = entries.map((entry) => ({
      type: entry.type,
      value: entry.value,
      valueType: entry.valueType,
      typeHint: entry.typeHint,
    }));
    // Use shouldValidate to ensure validation runs with the new values
    setValue(name, formEntries, {
      shouldDirty: true,
      shouldTouch: true,
      shouldValidate: true,
    });
  };

  // For StartScenario step types, use the SimpleInputMappingEditor with child scenario's input fields
  if (isStartScenario) {
    // If no child scenario selected yet, don't show the input mapping editor
    if (!childScenarioId) {
      return null;
    }

    // Convert current form data to initial data format
    const initialData = fieldArray.map((item: any) => ({
      type: item.type,
      value: item.value ?? '',
      valueType: item.valueType ?? 'immediate',
      typeHint: item.typeHint,
    }));

    // Use actual nodeId for edit mode, or a temporary ID for create mode
    const editorNodeId = nodeId || '__temp_create_node__';

    // If child scenario has no input fields, show a message
    if (childScenarioInputFields.length === 0 && fieldArray.length === 0) {
      return (
        <div>
          <div className="mb-4">{label}</div>
          <div className="text-sm text-muted-foreground border border-dashed rounded-lg p-4 text-center">
            The selected child scenario has no input parameters defined.
          </div>
        </div>
      );
    }

    return (
      <div>
        <div className="mb-4">{label}</div>
        <SimpleInputMappingEditor
          nodeId={editorNodeId}
          fields={childScenarioInputFields}
          initialData={initialData}
          onDataChange={handleSimpleEditorDataChange}
          allowCustomFields={true}
        />
        {error && (
          <div className="text-[0.8rem] mt-2 font-medium text-destructive">
            {error.message || error.root?.message}
          </div>
        )}
      </div>
    );
  }

  // Check if this capability has enhanced metadata (new format with CapabilityField[])
  const agent = agents?.find((a) => a.id === agentId);
  const capability = agent?.supportedCapabilities?.[capabilityId] as any;
  const hasEnhancedMetadata =
    capability &&
    Array.isArray(capability.inputs) &&
    capability.inputs.length > 0;

  // For Agent step types, use the SimpleInputMappingEditor
  // For create mode (no nodeId), use a temporary ID
  if (stepType === 'Agent') {
    // Filter out CONNECTION_DATA fields and connection_id if connection is selected
    // Handle case where capability.inputs may be undefined
    const capabilityInputs = hasEnhancedMetadata ? capability.inputs : [];
    const filteredInputs = capabilityInputs.filter((field: any) => {
      if (field.name.startsWith('get__CONNECTION_DATA')) {
        return false;
      }
      if (
        field.name === 'connection_id' &&
        connectionId &&
        connectionId !== '__none__'
      ) {
        return false;
      }
      return true;
    });

    // Convert current form data to initial data format
    // Include typeHint to ensure proper type conversion when saving
    const initialData = fieldArray.map((item: any) => ({
      type: item.type,
      value: item.value ?? '',
      valueType: item.valueType ?? 'immediate',
      typeHint: item.typeHint,
    }));

    // Use actual nodeId for edit mode, or a temporary ID for create mode
    const editorNodeId = nodeId || '__temp_create_node__';

    return (
      <div>
        <div className="mb-4">{label}</div>
        <SimpleInputMappingEditor
          nodeId={editorNodeId}
          fields={filteredInputs}
          initialData={initialData}
          onDataChange={handleSimpleEditorDataChange}
          allowCustomFields={true}
        />
        {error && (
          <div className="text-[0.8rem] mt-2 font-medium text-destructive">
            {error.message || error.root?.message}
          </div>
        )}
      </div>
    );
  }

  // For Error and other non-Agent step types that don't need input mapping
  // Just return null - these steps have their own dedicated form fields
  if (stepType === 'Error') {
    return null;
  }

  // No enhanced metadata available - show error
  return (
    <div>
      <div className="mb-4">{label}</div>
      <div className="border border-destructive/50 bg-destructive/10 rounded-lg p-4">
        <div className="flex items-start gap-3">
          <Icons.warning className="h-5 w-5 text-destructive shrink-0 mt-0.5" />
          <div className="space-y-1">
            <p className="text-sm font-medium text-destructive">
              Missing capability metadata
            </p>
            <p className="text-sm text-muted-foreground">
              The capability "{capabilityId}" for agent "{agentId}" does not
              have input field definitions. The backend must return an{' '}
              <code className="bg-muted px-1 rounded">inputs</code> array in the
              CapabilityInfo.
            </p>
          </div>
        </div>
      </div>
      {error && (
        <div className="text-[0.8rem] mt-2 font-medium text-destructive">
          {error.message || error.root?.message}
        </div>
      )}
    </div>
  );
}
