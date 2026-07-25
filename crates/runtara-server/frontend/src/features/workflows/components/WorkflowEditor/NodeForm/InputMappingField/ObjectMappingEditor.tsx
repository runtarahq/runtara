/**
 * ObjectMappingEditor - A specialized editor for object field mappings.
 *
 * Supports two modes:
 * 1. Reference mode: Map an entire object from a previous step
 * 2. Build mode: Build a structured object with mixed value types (composite)
 *
 * This component does NOT manage its own state - it derives fields from the value prop
 * and writes changes directly via onChange (which updates the Zustand store).
 */

import { useCallback, useMemo } from 'react';
import { Link, Layers, X } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { MappingValueInput } from './MappingValueInput';
import { CompositeValueEditor } from './CompositeValueEditor';
import {
  MappingModeToggle,
  type MappingModeToggleOption,
} from './MappingModeToggle';
import type {
  CompositeObjectValue,
  CompositeArrayValue,
  InputMappingValueType,
} from '@/features/workflows/stores/nodeFormStore';

type ObjectMode = 'reference' | 'build';

const MODE_OPTIONS: readonly MappingModeToggleOption<ObjectMode>[] = [
  { value: 'reference', label: 'Reference', icon: Link, tone: 'info' },
  { value: 'build', label: 'Build', icon: Layers },
];

interface ObjectMappingEditorProps {
  /** Current value (reference path for reference mode, or composite object for build mode) */
  value: string | CompositeObjectValue | CompositeArrayValue;
  /** Current value type */
  valueType: InputMappingValueType;
  /** Called when value changes */
  onChange: (
    value: string | CompositeObjectValue | CompositeArrayValue
  ) => void;
  /** Called when value type changes */
  onValueTypeChange: (type: InputMappingValueType) => void;
  /** Schema for typed objects (from field.items) - used for hints */
  schema?: {
    type?: string;
    properties?: Record<
      string,
      { type?: string; required?: boolean; description?: string }
    >;
    required?: string[];
  };
  /** Called when closing the object editor */
  onClose: () => void;
}

export function ObjectMappingEditor({
  value,
  valueType,
  onChange,
  onValueTypeChange,
  schema,
  onClose,
}: ObjectMappingEditorProps) {
  // Mode is derived from valueType: reference stays as reference, everything else is build (composite)
  const mode: ObjectMode = valueType === 'reference' ? 'reference' : 'build';

  // For build mode, get the composite value
  const compositeValue = useMemo(() => {
    if (mode !== 'build') return {};
    if (typeof value === 'object' && value !== null) {
      return value as CompositeObjectValue;
    }
    return {};
  }, [mode, value]);

  // Get schema fields for hints
  const schemaFields = useMemo(() => {
    if (!schema?.properties) return [];
    return Object.entries(schema.properties).map(([name, prop]) => ({
      name,
      type: prop.type || 'string',
      required: schema.required?.includes(name) || prop.required || false,
      description: prop.description,
    }));
  }, [schema]);

  const handleModeChange = (newMode: ObjectMode) => {
    if (newMode === 'reference') {
      onValueTypeChange('reference');
      onChange('');
    } else {
      onValueTypeChange('composite');
      onChange({});
    }
  };

  // Handle composite value changes
  const handleCompositeChange = useCallback(
    (newValue: CompositeObjectValue | CompositeArrayValue) => {
      onChange(newValue);
    },
    [onChange]
  );

  const handleClose = useCallback(() => {
    onClose();
  }, [onClose]);

  return (
    <div className="flex flex-col">
      {/* Mode selector with close button */}
      <div className="flex shrink-0 items-center gap-2 px-4 py-3">
        <MappingModeToggle
          className="flex-1"
          options={MODE_OPTIONS}
          value={mode}
          onChange={handleModeChange}
        />
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="size-9 shrink-0"
          onClick={handleClose}
        >
          <X className="size-4" />
        </Button>
      </div>

      {/* Content */}
      <div className="flex-1 overflow-y-auto p-4">
        {mode === 'reference' ? (
          // Reference mode - single reference input
          <div className="space-y-2">
            <p className="text-sm text-muted-foreground">
              Map an entire object from a previous step or trigger data.
            </p>
            <MappingValueInput
              value={typeof value === 'string' ? value : ''}
              onChange={(v) => onChange(v ?? '')}
              valueType="reference"
              onValueTypeChange={onValueTypeChange}
              fieldType="object"
              allowNull={false}
              placeholder="Select object reference..."
              hideReferenceToggle
            />
          </div>
        ) : (
          // Build mode - structured object with mixed value types
          <div className="space-y-2">
            <p className="text-sm text-muted-foreground">
              Build an object where each field can be an immediate value,
              reference, or nested object/array.
            </p>
            {/* Schema hint if available */}
            {schemaFields.length > 0 && (
              <div className="mb-2 flex flex-wrap items-center gap-2 rounded-md border bg-muted/30 p-2 text-xs">
                <span className="font-medium text-muted-foreground">
                  Expected fields:
                </span>
                {schemaFields.map((field) => (
                  <span
                    key={field.name}
                    className="flex items-center gap-1 rounded border bg-background px-2 py-0.5"
                  >
                    <span className="font-mono">{field.name}</span>
                    {field.required && (
                      <span className="text-destructive">*</span>
                    )}
                    <span className="text-muted-foreground">
                      ({field.type})
                    </span>
                  </span>
                ))}
              </div>
            )}
            <CompositeValueEditor
              value={compositeValue}
              onChange={handleCompositeChange}
              showModeSwitcher={false}
              showCloseButton={false}
            />
          </div>
        )}
      </div>
    </div>
  );
}
