/**
 * CompositeArrayEditor - Full-featured editor for composite array values.
 *
 * Provides a complete UI for building and editing array composites where
 * each item can be an immediate value, reference, or nested composite.
 *
 * Features:
 * - Add/remove items with type selection
 * - Type selector for each item
 * - Recursive editing for nested composites
 * - Validation display
 * - Item reordering (future enhancement)
 */

import { useCallback, useMemo } from 'react';
import {
  Plus,
  List,
  X,
  Link,
  Pin,
  Braces,
  Code,
  ChevronDown,
} from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/shared/components/ui/dropdown-menu';
import { cn } from '@/lib/utils';
import type {
  CompositeValue,
  CompositeArrayValue,
} from '@/features/scenarios/stores/nodeFormStore';
import { CompositeValueItem } from './CompositeValueItem';
import { VALUE_TYPE_OPTIONS } from '../ValueTypeSelector/constants';
import type { ValidationError } from './compositeValidation';

/** Icon components for value types */
const VALUE_TYPE_ICONS = {
  link: Link,
  pin: Pin,
  braces: Braces,
  list: List,
  code: Code,
} as const;

interface CompositeArrayEditorProps {
  /** The composite array value */
  value: CompositeArrayValue;
  /** Called when the value changes */
  onChange: (value: CompositeArrayValue) => void;
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

export function CompositeArrayEditor({
  value,
  onChange,
  onClose,
  depth = 0,
  validationErrors = [],
  pathPrefix = '',
  title = 'Composite Array',
  showCloseButton = true,
  disabled = false,
}: CompositeArrayEditorProps) {
  // Infer the default type for new items from the first element
  const inferredType = useMemo<
    'reference' | 'immediate' | 'composite-object' | 'composite-array'
  >(() => {
    if (value.length === 0) return 'immediate';
    const first = value[0];
    if (first.valueType === 'reference') return 'reference';
    if (first.valueType === 'composite') {
      return Array.isArray(first.value)
        ? 'composite-array'
        : 'composite-object';
    }
    return 'immediate';
  }, [value]);

  // Build a new item for the given type, preserving typeHint from the first element
  const buildNewItem = useCallback(
    (
      type:
        | 'reference'
        | 'immediate'
        | 'composite-object'
        | 'composite-array'
        | 'template'
    ): CompositeValue => {
      switch (type) {
        case 'reference':
          return { valueType: 'reference', value: '' };
        case 'immediate': {
          const firstItem = value[0];
          const typeHint =
            firstItem?.valueType === 'immediate'
              ? firstItem.typeHint
              : undefined;
          return typeHint
            ? { valueType: 'immediate', value: '', typeHint }
            : { valueType: 'immediate', value: '' };
        }
        case 'composite-object':
          return { valueType: 'composite', value: {} };
        case 'composite-array':
          return { valueType: 'composite', value: [] };
        default:
          return { valueType: 'immediate', value: '' };
      }
    },
    [value]
  );

  // Handle adding a new item with a specific type
  const handleAddItem = useCallback(
    (
      type:
        | 'reference'
        | 'immediate'
        | 'composite-object'
        | 'composite-array'
        | 'template'
    ) => {
      onChange([...value, buildNewItem(type)]);
    },
    [value, onChange, buildNewItem]
  );

  // Handle adding a new item using the inferred type from the first element
  const handleAddItemDefault = useCallback(() => {
    onChange([...value, buildNewItem(inferredType)]);
  }, [value, onChange, buildNewItem, inferredType]);

  // Handle updating an item's value
  const handleItemChange = useCallback(
    (index: number, newValue: CompositeValue) => {
      const newArray = [...value];
      newArray[index] = newValue;
      onChange(newArray);
    },
    [value, onChange]
  );

  // Handle removing an item
  const handleRemoveItem = useCallback(
    (index: number) => {
      onChange(value.filter((_, i) => i !== index));
    },
    [value, onChange]
  );

  // Handle moving an item up (commented out for future use)
  // const handleMoveUp = useCallback(
  //   (index: number) => {
  //     if (index === 0) return;
  //     const newArray = [...value];
  //     [newArray[index - 1], newArray[index]] = [newArray[index], newArray[index - 1]];
  //     onChange(newArray);
  //   },
  //   [value, onChange]
  // );

  // Handle moving an item down (commented out for future use)
  // const handleMoveDown = useCallback(
  //   (index: number) => {
  //     if (index === value.length - 1) return;
  //     const newArray = [...value];
  //     [newArray[index], newArray[index + 1]] = [newArray[index + 1], newArray[index]];
  //     onChange(newArray);
  //   },
  //   [value, onChange]
  // );

  // Get errors for a specific index
  const getItemErrors = useCallback(
    (index: number) => {
      const itemPath = pathPrefix ? `${pathPrefix}[${index}]` : `[${index}]`;
      return validationErrors.filter(
        (e) =>
          e.path === itemPath ||
          e.path.startsWith(itemPath + '.') ||
          e.path.startsWith(itemPath + '[')
      );
    },
    [validationErrors, pathPrefix]
  );

  return (
    <div className="flex flex-col">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 border-b bg-muted/20">
        <div className="flex items-center gap-2">
          <List className="h-4 w-4 text-green-600" />
          <span className="text-sm font-medium">{title}</span>
          <span className="text-xs text-muted-foreground">
            ({value.length} {value.length === 1 ? 'item' : 'items'})
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

      {/* Items */}
      <div className="flex-1 overflow-y-auto p-4 space-y-3">
        {value.length === 0 && (
          <div className="text-center py-8 text-muted-foreground">
            <List className="h-8 w-8 mx-auto mb-2 opacity-50" />
            <p className="text-sm">No items in array.</p>
            <p className="text-xs">Click "Add Item" to get started.</p>
          </div>
        )}

        {value.map((item, index) => (
          <div
            key={index}
            className={cn(
              'rounded-md border bg-background p-3',
              getItemErrors(index).length > 0 && 'border-destructive/50'
            )}
          >
            <CompositeValueItem
              label={`[${index}]`}
              value={item}
              onChange={(newValue) => handleItemChange(index, newValue)}
              onRemove={() => handleRemoveItem(index)}
              removable={!disabled}
              depth={depth}
              validationErrors={getItemErrors(index)}
              pathPrefix={pathPrefix ? `${pathPrefix}[${index}]` : `[${index}]`}
              disabled={disabled}
            />
          </div>
        ))}
      </div>

      {/* Footer with add button */}
      {!disabled && (
        <div className="px-4 py-3 border-t bg-muted/10">
          {value.length === 0 ? (
            /* First item: show dropdown to pick type */
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button
                  type="button"
                  variant="outline"
                  className="w-full border-dashed"
                >
                  <Plus className="h-4 w-4 mr-2" />
                  Add Item
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="center" className="w-56">
                {VALUE_TYPE_OPTIONS.map((option) => {
                  const IconComponent = VALUE_TYPE_ICONS[option.icon];
                  return (
                    <DropdownMenuItem
                      key={option.value}
                      onSelect={() => handleAddItem(option.value)}
                      className="cursor-pointer"
                    >
                      <IconComponent className="h-4 w-4 mr-2" />
                      <div className="flex flex-col">
                        <span>{option.label}</span>
                        <span className="text-xs text-muted-foreground">
                          {option.description}
                        </span>
                      </div>
                    </DropdownMenuItem>
                  );
                })}
              </DropdownMenuContent>
            </DropdownMenu>
          ) : (
            /* Subsequent items: direct add with first item's type, dropdown for other types */
            <div className="flex gap-1">
              <Button
                type="button"
                variant="outline"
                className="flex-1 border-dashed"
                onClick={handleAddItemDefault}
              >
                <Plus className="h-4 w-4 mr-2" />
                Add Item
              </Button>
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button
                    type="button"
                    variant="outline"
                    className="border-dashed px-2"
                  >
                    <ChevronDown className="h-4 w-4" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" className="w-56">
                  {VALUE_TYPE_OPTIONS.map((option) => {
                    const IconComponent = VALUE_TYPE_ICONS[option.icon];
                    return (
                      <DropdownMenuItem
                        key={option.value}
                        onSelect={() => handleAddItem(option.value)}
                        className="cursor-pointer"
                      >
                        <IconComponent className="h-4 w-4 mr-2" />
                        <div className="flex flex-col">
                          <span>{option.label}</span>
                          <span className="text-xs text-muted-foreground">
                            {option.description}
                          </span>
                        </div>
                      </DropdownMenuItem>
                    );
                  })}
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

// CompositeArrayEditor is used internally via named import
