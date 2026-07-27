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
  InputMappingValueType,
} from '@/features/workflows/stores/nodeFormStore';

type ObjectMode = 'value' | 'reference' | 'build';

const MODE_OPTIONS: readonly MappingModeToggleOption<ObjectMode>[] = [
  { value: 'reference', label: 'Reference', icon: Link, tone: 'info' },
  { value: 'build', label: 'Build', icon: Layers },
];

/**
 * An `any`-typed input accepts anything the DSL can express, so its editor has
 * to offer the whole shape choice — not just object-or-reference. 73 inputs
 * across 110 of 305 capabilities are `any`, including required ones like
 * `openai:create-embedding.input` and every HubSpot `filter_groups`.
 */
const UNTYPED_MODE_OPTIONS: readonly MappingModeToggleOption<ObjectMode>[] = [
  { value: 'value', label: 'Value', icon: Type },
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
  /**
   * The field's declared type is `any`/unknown, so the author picks the shape:
   * a plain value, a reference, or a built object *or array*. Typed object
   * fields keep the narrower reference-or-build choice.
   */
  untyped?: boolean;
}

export function ObjectMappingEditor({
  value,
  valueType,
  onChange,
  onValueTypeChange,
  schema,
  onClose,
  untyped = false,
}: ObjectMappingEditorProps) {
  // Mode is derived from valueType. For an untyped field a non-structural
  // immediate is a plain value; for a typed object field everything that is
  // not a reference is a build.
  const mode: ObjectMode =
    valueType === 'reference'
      ? 'reference'
      : untyped && !(typeof value === 'object' && value !== null)
        ? 'value'
        : 'build';

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
    } else if (newMode === 'value') {
      onValueTypeChange('immediate');
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
          options={untyped ? UNTYPED_MODE_OPTIONS : MODE_OPTIONS}
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
          // Plain value — the common case for an `any` input that just takes
          // a string, number or boolean.
          <div className="space-y-2">
            <p className="text-sm text-muted-foreground">
              Enter a value directly. Use Build for an object or a list.
            </p>
            <MappingValueInput
              value={typeof value === 'object' && value !== null ? '' : value}
              onChange={(v) => onChange(v ?? '')}
              valueType="immediate"
              onValueTypeChange={onValueTypeChange}
              fieldType="text"
              allowNull
              hideReferenceToggle
              placeholder="Enter a value..."
            />
          </div>
        ) : mode === 'reference' ? (
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
            {/* No standing description: what Build mode does is spelled out by
                the editor's own empty state, which is on screen exactly when
                the explanation is useful. Once fields exist it is a permanent
                two-line header restating what the fields already show. */}
            {/* Schema hint if available */}
            {schemaFields.length > 0 && (
              <div className="mb-2 flex flex-wrap items-center gap-2 rounded-md border bg-muted/30 p-2 text-xs">
                <span className="font-medium text-muted-foreground">
                  Expected fields:
                </span>
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
              </div>
            )}
            <CompositeValueEditor
              value={compositeValue}
              onChange={handleCompositeChange}
              // An untyped root may legitimately be an array; a typed object
              // field may not, so only `any` gets the object/array switcher.
              showModeSwitcher={untyped}
              showCloseButton={false}
            />
          </div>
        )}
      </div>
    </div>
  );
}
