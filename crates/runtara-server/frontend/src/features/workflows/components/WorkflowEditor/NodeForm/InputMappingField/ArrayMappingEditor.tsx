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

import { useCallback, useMemo, useState } from 'react';
import { Link, Layers, Type, X } from 'lucide-react';
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
  InputMappingEntry,
  InputMappingValueType,
} from '@/features/workflows/stores/nodeFormStore';
import { JSON_TYPE_HINT, isJsonLiteral, jsonLiteralText } from './json-literal';

type ArrayMode = 'value' | 'reference' | 'build';

const MODE_OPTIONS: readonly MappingModeToggleOption<ArrayMode>[] = [
  { value: 'reference', label: 'Reference', icon: Link, tone: 'info' },
  { value: 'build', label: 'Build', icon: Layers },
];

/** Shown while the field holds a literal list, so it has somewhere to be seen. */
const LITERAL_MODE_OPTIONS: readonly MappingModeToggleOption<ArrayMode>[] = [
  { value: 'value', label: 'Value', icon: Type },
  ...MODE_OPTIONS,
];

interface ArrayMappingEditorProps {
  /** The array type (e.g., "array<string>", "array<object>") - used for hints */
  arrayType: string;
  /**
   * Current value: a reference path, a composite array for build mode, or an
   * immediate list held as one literal.
   */
  value: InputMappingEntry['value'];
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
  /** The entry's type hint; `json` marks the text of a JSON literal. */
  typeHint?: string;
  /**
   * Set the entry's type hint: `json` when a JSON literal is edited as text,
   * `undefined` to return to the field's own hint on a mode switch.
   */
  onTypeHintChange?: (typeHint: string | undefined) => void;
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
  typeHint,
  onTypeHintChange,
}: ArrayMappingEditorProps) {
  // Set once the author edits a JSON literal, so clearing the text to retype
  // it does not drop the field back to an empty Build.
  const [editingJson, setEditingJson] = useState(false);

  // Mode is derived from the stored entry: an immediate holding a list is one
  // literal value (see json-literal.ts), anything else that is not a
  // reference is a build.
  const mode: ArrayMode =
    valueType === 'reference'
      ? 'reference'
      : isJsonLiteral(value, valueType, typeHint, false) ||
          (editingJson && valueType === 'immediate')
        ? 'value'
        : 'build';

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
    // Clicking the lit segment is not a request to start over: every branch
    // below resets the value.
    if (newMode === mode) return;
    setEditingJson(false);
    if (typeHint === JSON_TYPE_HINT) onTypeHintChange?.(undefined);
    if (newMode === 'reference') {
      onValueTypeChange('reference');
      onChange('');
    } else if (newMode === 'value') {
      onValueTypeChange('immediate');
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
      <div className="flex shrink-0 items-center gap-2 px-4 py-3">
        <MappingModeToggle
          className="flex-1"
          options={mode === 'value' ? LITERAL_MODE_OPTIONS : MODE_OPTIONS}
          value={mode}
          onChange={handleModeChange}
        />
        <Button
          type="button"
          variant="secondary"
          size="icon"
          className="size-9 shrink-0"
          onClick={handleClose}
        >
          <X className="size-4" />
        </Button>
      </div>

      {/* Content */}
      <div className="flex-1 overflow-y-auto p-4">
        {mode === 'value' ? (
          <div className="space-y-2">
            <p className="text-sm text-muted-foreground">
              A JSON value, sent as written.
            </p>
            <MappingValueInput
              value={jsonLiteralText(value)}
              onChange={(v) => {
                setEditingJson(true);
                if (typeHint !== JSON_TYPE_HINT) {
                  onTypeHintChange?.(JSON_TYPE_HINT);
                }
                onChange(v ?? '');
              }}
              valueType="immediate"
              onValueTypeChange={onValueTypeChange}
              fieldType="json"
              hideReferenceToggle
              placeholder="Enter a JSON value..."
            />
          </div>
        ) : mode === 'reference' ? (
          // Reference mode - single reference input
          <div className="space-y-2">
            <p className="text-sm text-muted-foreground">
              Map an entire array from a previous step or trigger data.
            </p>
            <MappingValueInput
              value={typeof value === 'string' ? value : ''}
              onChange={(v) => onChange(v ?? '')}
              valueType="reference"
              onValueTypeChange={onValueTypeChange}
              fieldType="array"
              allowNull={false}
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
              <div className="mb-2 flex flex-wrap items-center gap-2 rounded-md border bg-muted/30 p-2 text-xs">
                <span className="font-medium text-muted-foreground">
                  Item type: <span className="font-mono">{itemType}</span>
                </span>
                {schemaFields.length > 0 && (
                  <>
                    <span className="text-muted-foreground">|</span>
                    <span className="text-muted-foreground">Fields:</span>
                    {schemaFields.map((field) => (
                      <span
                        key={field.name}
                        className="flex items-center gap-1 rounded-full border bg-background px-2 py-0.5"
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
