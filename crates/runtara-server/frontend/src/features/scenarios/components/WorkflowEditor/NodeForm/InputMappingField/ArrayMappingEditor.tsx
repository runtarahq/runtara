/**
 * ArrayMappingEditor - A specialized editor for array field mappings.
 *
 * Supports two modes:
 * 1. Reference mode: Map an entire array from a previous step
 * 2. Build mode: Build a structured array with mixed value types (composite)
 *
 * This component does NOT manage its own state - it derives items from the value prop
 * and writes changes directly via onChange (which updates the Zustand store).
 */

import { useCallback, useMemo } from 'react';
import { Link, Layers, X } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { cn } from '@/lib/utils';
import { MappingValueInput } from './MappingValueInput';
import { CompositeValueEditor } from './CompositeValueEditor';
import type {
  CompositeObjectValue,
  CompositeArrayValue,
  InputMappingValueType,
} from '@/features/scenarios/stores/nodeFormStore';

type ArrayMode = 'reference' | 'build';

interface ArrayMappingEditorProps {
  /** The array type (e.g., "array<string>", "array<object>") - used for hints */
  arrayType: string;
  /** Current value (reference path for reference mode, or composite array for build mode) */
  value: string | CompositeObjectValue | CompositeArrayValue;
  /** Current value type */
  valueType: InputMappingValueType;
  /** Called when value changes */
  onChange: (
    value: string | CompositeObjectValue | CompositeArrayValue
  ) => void;
  /** Called when value type changes */
  onValueTypeChange: (type: InputMappingValueType) => void;
  /** Item schema for typed object arrays - used for hints */
  itemSchema?: {
    type?: string;
    properties?: Record<
      string,
      { type?: string; required?: boolean; description?: string }
    >;
    required?: string[];
  };
  /** Called when closing the array editor */
  onClose: () => void;
}

/** Parse the inner type from array type string */
function parseArrayItemType(arrayType: string): string {
  const match = arrayType.match(/array<(.+)>|^\[(.+)\]$/i);
  if (match) {
    return match[1] || match[2] || 'any';
  }
  return 'any';
}

export function ArrayMappingEditor({
  arrayType,
  value,
  valueType,
  onChange,
  onValueTypeChange,
  itemSchema,
  onClose,
}: ArrayMappingEditorProps) {
  // Mode is derived from valueType: reference stays as reference, everything else is build (composite)
  const mode: ArrayMode = valueType === 'reference' ? 'reference' : 'build';

  // For build mode, get the composite value
  const compositeValue = useMemo(() => {
    if (mode !== 'build') return [];
    if (Array.isArray(value)) {
      return value as CompositeArrayValue;
    }
    return [];
  }, [mode, value]);

  // Parse the item type from array type for hints
  const itemType = parseArrayItemType(arrayType);

  // Get schema fields for hints
  const schemaFields = itemSchema?.properties
    ? Object.entries(itemSchema.properties).map(([name, prop]) => ({
        name,
        type: prop.type || 'string',
        required: itemSchema.required?.includes(name) || prop.required || false,
        description: prop.description,
      }))
    : [];

  const handleModeChange = (newMode: ArrayMode) => {
    if (newMode === 'reference') {
      onValueTypeChange('reference');
      onChange('');
    } else {
      onValueTypeChange('composite');
      onChange([]);
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
      <div className="flex items-center gap-2 px-4 py-3 shrink-0">
        <div className="flex gap-1 flex-1">
          <button
            type="button"
            onClick={() => handleModeChange('reference')}
            className={cn(
              'flex-1 flex items-center justify-center gap-1.5 px-3 py-2',
              'text-sm border rounded-md transition-colors',
              mode === 'reference'
                ? 'bg-blue-50 border-blue-300 text-blue-700 dark:bg-blue-950 dark:border-blue-800 dark:text-blue-400'
                : 'bg-background border-input text-muted-foreground hover:bg-muted/50'
            )}
          >
            <Link className="h-4 w-4" />
            Reference
          </button>
          <button
            type="button"
            onClick={() => handleModeChange('build')}
            className={cn(
              'flex-1 flex items-center justify-center gap-1.5 px-3 py-2',
              'text-sm border rounded-md transition-colors',
              mode === 'build'
                ? 'bg-green-50 border-green-300 text-green-700 dark:bg-green-950 dark:border-green-800 dark:text-green-400'
                : 'bg-background border-input text-muted-foreground hover:bg-muted/50'
            )}
          >
            <Layers className="h-4 w-4" />
            Build
          </button>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="h-9 w-9 shrink-0"
          onClick={handleClose}
        >
          <X className="h-4 w-4" />
        </Button>
      </div>

      {/* Content */}
      <div className="flex-1 overflow-y-auto p-4">
        {mode === 'reference' ? (
          // Reference mode - single reference input
          <div className="space-y-2">
            <p className="text-sm text-muted-foreground">
              Map an entire array from a previous step or trigger data.
            </p>
            <MappingValueInput
              value={typeof value === 'string' ? value : ''}
              onChange={(v) => onChange(v)}
              valueType="reference"
              onValueTypeChange={onValueTypeChange}
              fieldType="array"
              placeholder="Select array reference..."
              hideReferenceToggle
            />
          </div>
        ) : (
          // Build mode - structured array with mixed value types
          <div className="space-y-2">
            <p className="text-sm text-muted-foreground">
              Build an array where each item can be an immediate value,
              reference, or nested object/array.
            </p>
            {/* Item type hint */}
            {(itemType !== 'any' || schemaFields.length > 0) && (
              <div className="flex items-center gap-2 p-2 bg-muted/30 rounded-md border text-xs flex-wrap mb-2">
                <span className="text-muted-foreground font-medium">
                  Item type: <span className="font-mono">{itemType}</span>
                </span>
                {schemaFields.length > 0 && (
                  <>
                    <span className="text-muted-foreground">|</span>
                    <span className="text-muted-foreground">Fields:</span>
                    {schemaFields.map((field) => (
                      <span
                        key={field.name}
                        className="flex items-center gap-1 px-2 py-0.5 bg-background rounded border"
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
                  </>
                )}
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
