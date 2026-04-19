/**
 * CompositeObjectEditor - Full-featured editor for composite object values.
 *
 * Provides a complete UI for building and editing object composites where
 * each field can be an immediate value, reference, or nested composite.
 *
 * Features:
 * - Add/remove fields with custom names
 * - Type selector for each field
 * - Recursive editing for nested composites
 * - Validation display
 * - Expandable/collapsible tree structure
 */

import { useState, useCallback } from 'react';
import { Plus, Braces, X } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import { cn } from '@/lib/utils';
import type {
  CompositeValue,
  CompositeObjectValue,
} from '@/features/workflows/stores/nodeFormStore';
import { CompositeValueItem } from './CompositeValueItem';
import type { ValidationError } from './compositeValidation';

interface CompositeObjectEditorProps {
  /** The composite object value */
  value: CompositeObjectValue;
  /** Called when the value changes */
  onChange: (value: CompositeObjectValue) => void;
  /** Called when the editor should be closed */
  onClose?: () => void;
  /** Current nesting depth */
  depth?: number;
  /** Validation errors */
  validationErrors?: ValidationError[];
  /** Path prefix for error matching */
  pathPrefix?: string;
  /** Title to display */
  title?: string;
  /** Whether to show the close button */
  showCloseButton?: boolean;
  /** Disable all editing */
  disabled?: boolean;
}

export function CompositeObjectEditor({
  value,
  onChange,
  onClose,
  depth = 0,
  validationErrors = [],
  pathPrefix = '',
  title = 'Composite Object',
  showCloseButton = true,
  disabled = false,
}: CompositeObjectEditorProps) {
  const [newFieldName, setNewFieldName] = useState('');
  const [isAddingField, setIsAddingField] = useState(false);

  const fieldNames = Object.keys(value);

  // Debug: log when component renders with value
  console.log('[CompositeObjectEditor] render', {
    fieldNames,
    value,
    fieldCount: fieldNames.length,
  });

  // Handle adding a new field
  const handleAddField = useCallback(() => {
    const trimmedName = newFieldName.trim();
    console.log('[CompositeObjectEditor] handleAddField called', {
      trimmedName,
      currentValue: value,
      fieldCount: Object.keys(value).length,
    });
    if (!trimmedName) {
      console.log(
        '[CompositeObjectEditor] handleAddField: empty name, returning'
      );
      return;
    }

    // Check for duplicate field names
    if (value[trimmedName]) {
      console.log(
        '[CompositeObjectEditor] handleAddField: duplicate field name, returning'
      );
      // Could show an error toast here
      return;
    }

    const newValue = {
      ...value,
      [trimmedName]: { valueType: 'immediate' as const, value: '' },
    };
    console.log(
      '[CompositeObjectEditor] handleAddField: calling onChange with',
      newValue
    );
    onChange(newValue);
    setNewFieldName('');
    setIsAddingField(false);
  }, [newFieldName, value, onChange]);

  // Handle updating a field's value
  const handleFieldChange = useCallback(
    (fieldName: string, newValue: CompositeValue) => {
      onChange({
        ...value,
        [fieldName]: newValue,
      });
    },
    [value, onChange]
  );

  // Handle removing a field
  const handleRemoveField = useCallback(
    (fieldName: string) => {
      const { [fieldName]: _removed, ...rest } = value;
      void _removed; // Intentionally unused - destructuring to remove field
      onChange(rest);
    },
    [value, onChange]
  );

  // Handle renaming a field (commented out for future use)
  // const handleRenameField = useCallback(
  //   (oldName: string, newName: string) => {
  //     const trimmedNewName = newName.trim();
  //     if (!trimmedNewName || trimmedNewName === oldName) return;
  //     if (value[trimmedNewName]) return; // Duplicate
  //
  //     const entries = Object.entries(value);
  //     const newEntries = entries.map(([key, val]) =>
  //       key === oldName ? [trimmedNewName, val] : [key, val]
  //     );
  //     onChange(Object.fromEntries(newEntries));
  //   },
  //   [value, onChange]
  // );

  // Get errors for a specific field
  const getFieldErrors = useCallback(
    (fieldName: string) => {
      const fieldPath = pathPrefix ? `${pathPrefix}.${fieldName}` : fieldName;
      return validationErrors.filter(
        (e) => e.path === fieldPath || e.path.startsWith(fieldPath + '.')
      );
    },
    [validationErrors, pathPrefix]
  );

  return (
    <div className="flex flex-col">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 border-b bg-muted/20">
        <div className="flex items-center gap-2">
          <Braces className="h-4 w-4 text-green-600" />
          <span className="text-sm font-medium">{title}</span>
          <span className="text-xs text-muted-foreground">
            ({fieldNames.length} {fieldNames.length === 1 ? 'field' : 'fields'})
          </span>
        </div>
        {showCloseButton && onClose && (
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="h-8 w-8"
            onClick={onClose}
          >
            <X className="h-4 w-4" />
          </Button>
        )}
      </div>

      {/* Fields */}
      <div className="flex-1 overflow-y-auto p-4 space-y-3">
        {fieldNames.length === 0 && !isAddingField && (
          <div className="text-center py-8 text-muted-foreground">
            <Braces className="h-8 w-8 mx-auto mb-2 opacity-50" />
            <p className="text-sm">No fields defined yet.</p>
            <p className="text-xs">Click "Add Field" to get started.</p>
          </div>
        )}

        {fieldNames.map((fieldName) => {
          console.log('[CompositeObjectEditor] rendering field', {
            fieldName,
            fieldValue: value[fieldName],
          });
          return (
            <div
              key={fieldName}
              className={cn(
                'rounded-md border bg-background p-3',
                getFieldErrors(fieldName).length > 0 && 'border-destructive/50'
              )}
            >
              <CompositeValueItem
                label={fieldName}
                value={value[fieldName]}
                onChange={(newValue) => handleFieldChange(fieldName, newValue)}
                onRemove={() => handleRemoveField(fieldName)}
                removable={!disabled}
                depth={depth}
                validationErrors={getFieldErrors(fieldName)}
                pathPrefix={
                  pathPrefix ? `${pathPrefix}.${fieldName}` : fieldName
                }
                disabled={disabled}
              />
            </div>
          );
        })}

        {/* Add field form */}
        {isAddingField && (
          <div className="rounded-md border border-dashed bg-muted/30 p-3">
            <div className="flex items-center gap-2">
              <Input
                type="text"
                value={newFieldName}
                onChange={(e) => setNewFieldName(e.target.value)}
                onKeyDown={(e) => {
                  console.log('[CompositeObjectEditor] Input onKeyDown', {
                    key: e.key,
                    newFieldName,
                  });
                  if (e.key === 'Enter') {
                    e.preventDefault(); // Prevent form submission
                    handleAddField();
                  }
                  if (e.key === 'Escape') {
                    setIsAddingField(false);
                    setNewFieldName('');
                  }
                }}
                placeholder="Enter field name..."
                className="flex-1 h-8"
                autoFocus
              />
              <Button
                type="button"
                size="sm"
                onClick={handleAddField}
                disabled={!newFieldName.trim()}
              >
                Add
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={() => {
                  setIsAddingField(false);
                  setNewFieldName('');
                }}
              >
                Cancel
              </Button>
            </div>
          </div>
        )}
      </div>

      {/* Footer with add button */}
      {!disabled && (
        <div className="px-4 py-3 border-t bg-muted/10">
          <Button
            type="button"
            variant="outline"
            className="w-full border-dashed"
            onClick={() => setIsAddingField(true)}
            disabled={isAddingField}
          >
            <Plus className="h-4 w-4 mr-2" />
            Add Field
          </Button>
        </div>
      )}
    </div>
  );
}

// CompositeObjectEditor is used internally via named import
