/**
 * CompositeValueItem - Renders a single value within a composite structure.
 *
 * This component handles:
 * - Immediate values: Shows inline editable input
 * - Reference values: Shows reference pill with variable picker
 * - Composite values: Shows expandable nested editor
 *
 * Supports recursive rendering for arbitrarily nested composites.
 */

import { useState, useContext, useMemo, useCallback } from 'react';
import {
  Link,
  Pin,
  Braces,
  List,
  Code,
  ChevronDown,
  ChevronRight,
  AlertTriangle,
  X,
} from 'lucide-react';
import { Input } from '@/shared/components/ui/input';
import { Button } from '@/shared/components/ui/button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/shared/components/ui/dropdown-menu';
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/shared/components/ui/tooltip';
import { cn } from '@/lib/utils';
import type {
  CompositeValue,
  CompositeObjectValue,
  CompositeArrayValue,
  CompositeImmediateTypeHint,
} from '@/features/workflows/stores/nodeFormStore';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { Checkbox } from '@/shared/components/ui/checkbox';
import { ReferencePill } from './ReferencePill';
import { VariablePickerModal } from './VariablePickerModal';
import { VariableSuggestion } from '../InputMappingValueField/VariableSuggestions';
import { NodeFormContext } from '../NodeFormContext';
import {
  VALUE_TYPE_OPTIONS,
  getValueTypeBadgeColor,
} from '../ValueTypeSelector/constants';
import type { ValidationError } from './compositeValidation';

// Forward declaration for recursive component
interface CompositeObjectEditorProps {
  value: CompositeObjectValue;
  onChange: (value: CompositeObjectValue) => void;
  depth?: number;
  validationErrors?: ValidationError[];
}

interface CompositeArrayEditorProps {
  value: CompositeArrayValue;
  onChange: (value: CompositeArrayValue) => void;
  depth?: number;
  validationErrors?: ValidationError[];
}

interface CompositeValueItemProps {
  /** The value to render */
  value: CompositeValue;
  /** Called when the value changes */
  onChange: (value: CompositeValue) => void;
  /** Called when the item should be removed */
  onRemove?: () => void;
  /** Label to display (e.g., field name or array index) */
  label?: string;
  /** Whether this is a removable item */
  removable?: boolean;
  /** Current nesting depth (for visual indentation limits) */
  depth?: number;
  /** Validation errors for this item and its children */
  validationErrors?: ValidationError[];
  /** Path prefix for error matching */
  pathPrefix?: string;
  /** Whether to hide the type selector */
  hideTypeSelector?: boolean;
  /** Disable editing */
  disabled?: boolean;
}

/** Maximum nesting depth for visual indentation */
const MAX_VISUAL_DEPTH = 6;

/** Icon components for value types */
const VALUE_TYPE_ICONS = {
  link: Link,
  pin: Pin,
  braces: Braces,
  list: List,
  code: Code,
} as const;

/** Type hint options for immediate values */
const IMMEDIATE_TYPE_HINT_OPTIONS: Array<{
  value: CompositeImmediateTypeHint;
  label: string;
  description: string;
}> = [
  { value: 'auto', label: 'Auto', description: 'Automatically detect type' },
  { value: 'string', label: 'String', description: 'Text value' },
  { value: 'integer', label: 'Integer', description: 'Whole number' },
  { value: 'number', label: 'Number', description: 'Decimal number' },
  { value: 'boolean', label: 'Boolean', description: 'True/False' },
  { value: 'file', label: 'File', description: 'File reference' },
  { value: 'json', label: 'JSON', description: 'JSON object/array' },
];

/**
 * Extract step ID from a reference path like "steps['stepId'].outputs.field"
 */
function extractStepIdFromPath(path: string): string | null {
  const match = path.match(/steps\['([^']+)'\]/);
  return match ? match[1] : null;
}

/**
 * Extract field path from a reference path
 */
function extractFieldPathFromPath(path: string): string | null {
  const match = path.match(/\.outputs\.?(.*)$/);
  return match ? match[1] || 'outputs' : null;
}

export function CompositeValueItem({
  value,
  onChange,
  onRemove,
  label,
  removable = true,
  depth = 0,
  validationErrors = [],
  pathPrefix = '',
  hideTypeSelector = false,
  disabled = false,
}: CompositeValueItemProps) {
  const [isPickerOpen, setIsPickerOpen] = useState(false);
  const [isExpanded, setIsExpanded] = useState(true);
  const { previousSteps } = useContext(NodeFormContext);

  // Get errors for this specific item
  const itemErrors = useMemo(() => {
    return validationErrors.filter(
      (error) =>
        error.path === pathPrefix || error.path.startsWith(pathPrefix + '.')
    );
  }, [validationErrors, pathPrefix]);

  const hasError = itemErrors.length > 0;
  const directError = itemErrors.find((e) => e.path === pathPrefix);

  // For reference values, look up step info
  const stepInfo = useMemo(() => {
    if (value.valueType !== 'reference' || !value.value) {
      return { stepName: undefined, stepId: undefined, fieldPath: undefined };
    }
    const stepId = extractStepIdFromPath(value.value as string);
    if (!stepId) {
      return { stepName: undefined, stepId: undefined, fieldPath: undefined };
    }
    const step = previousSteps.find((s) => s.id === stepId);
    const fieldPath = extractFieldPathFromPath(value.value as string);
    return {
      stepName: step?.name,
      stepId,
      fieldPath,
    };
  }, [value, previousSteps]);

  // Handle value type change
  const handleTypeChange = useCallback(
    (
      newType:
        | 'reference'
        | 'immediate'
        | 'composite-object'
        | 'composite-array'
        | 'template'
    ) => {
      if (newType === 'reference') {
        onChange({ valueType: 'reference', value: '' });
        setIsPickerOpen(true);
      } else if (newType === 'immediate') {
        onChange({ valueType: 'immediate', value: '' });
      } else if (newType === 'composite-object') {
        onChange({ valueType: 'composite', value: {} });
      } else if (newType === 'composite-array') {
        onChange({ valueType: 'composite', value: [] });
      } else if (newType === 'template') {
        onChange({ valueType: 'immediate', value: '' });
      }
    },
    [onChange]
  );

  // Handle variable selection from picker
  const handleVariableSelect = useCallback(
    (variable: VariableSuggestion) => {
      onChange({ valueType: 'reference', value: variable.value });
    },
    [onChange]
  );

  // Get current type hint (only for immediate values)
  const currentTypeHint: CompositeImmediateTypeHint =
    value.valueType === 'immediate' && value.typeHint ? value.typeHint : 'auto';

  // Handle immediate value change
  const handleImmediateChange = useCallback(
    (inputValue: string) => {
      // Preserve existing typeHint when changing value
      const existingTypeHint =
        value.valueType === 'immediate' && value.typeHint
          ? value.typeHint
          : undefined;

      // Parse value based on typeHint
      let parsedValue: string | number | boolean = inputValue;
      if (existingTypeHint === 'boolean') {
        parsedValue = inputValue === 'true';
      } else if (
        existingTypeHint === 'integer' ||
        existingTypeHint === 'number'
      ) {
        // Keep as string but it will be converted on serialization
        parsedValue = inputValue;
      } else if (existingTypeHint === 'auto' || !existingTypeHint) {
        // Auto-detect: try to parse as number or boolean
        if (inputValue === 'true') parsedValue = true;
        else if (inputValue === 'false') parsedValue = false;
        else if (inputValue !== '' && !isNaN(Number(inputValue))) {
          parsedValue = Number(inputValue);
        }
      }

      const newValue: CompositeValue = {
        valueType: 'immediate',
        value: parsedValue,
      };
      if (existingTypeHint && existingTypeHint !== 'auto') {
        (
          newValue as {
            valueType: 'immediate';
            value: any;
            typeHint?: CompositeImmediateTypeHint;
          }
        ).typeHint = existingTypeHint;
      }
      onChange(newValue);
    },
    [onChange, value]
  );

  // Handle type hint change
  const handleTypeHintChange = useCallback(
    (newTypeHint: CompositeImmediateTypeHint) => {
      if (value.valueType !== 'immediate') return;

      const newValue: CompositeValue = {
        valueType: 'immediate',
        value: value.value,
      };
      if (newTypeHint !== 'auto') {
        (
          newValue as {
            valueType: 'immediate';
            value: any;
            typeHint?: CompositeImmediateTypeHint;
          }
        ).typeHint = newTypeHint;
      }
      onChange(newValue);
    },
    [onChange, value]
  );

  // Handle nested composite changes
  const handleCompositeChange = useCallback(
    (newValue: CompositeObjectValue | CompositeArrayValue) => {
      onChange({ valueType: 'composite', value: newValue });
    },
    [onChange]
  );

  // Render the type badge
  const renderTypeBadge = () => {
    const isComposite = value.valueType === 'composite';
    const isArray = isComposite && Array.isArray(value.value);
    const badgeType = isComposite
      ? isArray
        ? 'composite-array'
        : 'composite-object'
      : value.valueType;
    const IconComponent =
      VALUE_TYPE_ICONS[
        badgeType === 'reference'
          ? 'link'
          : badgeType === 'immediate'
            ? 'pin'
            : isArray
              ? 'list'
              : 'braces'
      ];

    return (
      <span
        className={cn(
          'inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-xs border',
          getValueTypeBadgeColor(badgeType)
        )}
      >
        <IconComponent className="h-3 w-3" />
        <span className="sr-only">{badgeType}</span>
      </span>
    );
  };

  // Render type-specific input for immediate values
  const renderImmediateInput = () => {
    const typeHint = currentTypeHint;

    // Boolean input - checkbox
    if (typeHint === 'boolean') {
      const boolValue = value.value === true || value.value === 'true';
      return (
        <div className="flex items-center h-8 px-2">
          <Checkbox
            checked={boolValue}
            onCheckedChange={(checked) =>
              handleImmediateChange(String(checked))
            }
            disabled={disabled}
          />
          <span className="ml-2 text-sm text-muted-foreground">
            {boolValue ? 'True' : 'False'}
          </span>
        </div>
      );
    }

    // Number input for int/double
    if (typeHint === 'integer' || typeHint === 'number') {
      return (
        <Input
          type="number"
          value={String(value.value ?? '')}
          onChange={(e) => handleImmediateChange(e.target.value)}
          placeholder={
            typeHint === 'integer' ? 'Enter integer...' : 'Enter number...'
          }
          step={typeHint === 'integer' ? '1' : 'any'}
          disabled={disabled}
          className={cn('flex-1 h-8 text-sm', hasError && 'border-destructive')}
        />
      );
    }

    // Default text input for string, json, file, auto
    return (
      <Input
        type="text"
        value={String(value.value ?? '')}
        onChange={(e) => handleImmediateChange(e.target.value)}
        placeholder={
          typeHint === 'json'
            ? 'Enter JSON...'
            : typeHint === 'file'
              ? 'Enter file path...'
              : 'Enter value...'
        }
        disabled={disabled}
        className={cn(
          'flex-1 h-8 text-sm',
          typeHint === 'json' && 'font-mono',
          hasError && 'border-destructive'
        )}
      />
    );
  };

  // Render the value editor based on type
  const renderValueEditor = () => {
    switch (value.valueType) {
      case 'immediate':
        return (
          <div className="flex items-center gap-2 flex-1">
            {/* Type hint selector */}
            <Select
              value={currentTypeHint}
              onValueChange={(val) =>
                handleTypeHintChange(val as CompositeImmediateTypeHint)
              }
              disabled={disabled}
            >
              <SelectTrigger className="h-8 w-[90px] text-xs shrink-0">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {IMMEDIATE_TYPE_HINT_OPTIONS.map((option) => (
                  <SelectItem key={option.value} value={option.value}>
                    <div className="flex flex-col">
                      <span>{option.label}</span>
                    </div>
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            {/* Value input */}
            {renderImmediateInput()}
          </div>
        );

      case 'reference':
        if (value.value) {
          return (
            <div className="flex-1">
              <ReferencePill
                path={value.value as string}
                stepName={stepInfo.stepName}
                fieldPath={stepInfo.fieldPath ?? undefined}
                onRemove={() => onChange({ valueType: 'reference', value: '' })}
                disabled={disabled}
                className={hasError ? 'border-destructive' : undefined}
              />
            </div>
          );
        }
        return (
          <button
            type="button"
            onClick={() => setIsPickerOpen(true)}
            disabled={disabled}
            className={cn(
              'flex-1 flex items-center justify-center h-8 px-3',
              'text-sm text-muted-foreground',
              'bg-muted/30 border border-dashed rounded-md',
              'hover:bg-muted/50 hover:border-muted-foreground/50 transition-colors',
              disabled && 'opacity-50 cursor-not-allowed',
              hasError && 'border-destructive'
            )}
          >
            Click to select variable...
          </button>
        );

      case 'composite': {
        const compositeValue = value.value as
          | CompositeObjectValue
          | CompositeArrayValue;
        const isArray = Array.isArray(compositeValue);
        const itemCount = isArray
          ? compositeValue.length
          : Object.keys(compositeValue).length;

        return (
          <div className="flex-1">
            <button
              type="button"
              onClick={() => setIsExpanded(!isExpanded)}
              className={cn(
                'flex items-center gap-2 text-sm text-muted-foreground',
                'hover:text-foreground transition-colors'
              )}
            >
              {isExpanded ? (
                <ChevronDown className="h-4 w-4" />
              ) : (
                <ChevronRight className="h-4 w-4" />
              )}
              <span>
                {isArray
                  ? itemCount === 1
                    ? '1 item'
                    : `${itemCount} items`
                  : itemCount === 1
                    ? '1 field'
                    : `${itemCount} fields`}
              </span>
            </button>
          </div>
        );
      }

      default:
        return <span className="text-muted-foreground">Unknown type</span>;
    }
  };

  // Render nested composite editor
  const renderNestedEditor = () => {
    if (value.valueType !== 'composite' || !isExpanded) return null;

    const compositeValue = value.value as
      | CompositeObjectValue
      | CompositeArrayValue;
    const isArray = Array.isArray(compositeValue);
    const visualDepth = Math.min(depth + 1, MAX_VISUAL_DEPTH);

    // Import nested editors dynamically to avoid circular dependencies
    // For now, we'll render inline versions
    if (isArray) {
      return (
        <div
          className={cn(
            'mt-2 ml-4 pl-3 border-l-2 border-muted',
            visualDepth >= MAX_VISUAL_DEPTH && 'ml-2 pl-2'
          )}
        >
          <CompositeArrayEditorInline
            value={compositeValue}
            onChange={
              handleCompositeChange as (value: CompositeArrayValue) => void
            }
            depth={depth + 1}
            validationErrors={itemErrors}
            pathPrefix={pathPrefix}
          />
        </div>
      );
    }

    return (
      <div
        className={cn(
          'mt-2 ml-4 pl-3 border-l-2 border-muted',
          visualDepth >= MAX_VISUAL_DEPTH && 'ml-2 pl-2'
        )}
      >
        <CompositeObjectEditorInline
          value={compositeValue}
          onChange={
            handleCompositeChange as (value: CompositeObjectValue) => void
          }
          depth={depth + 1}
          validationErrors={itemErrors}
          pathPrefix={pathPrefix}
        />
      </div>
    );
  };

  return (
    <div className="space-y-0">
      <div className="flex items-center gap-2">
        {/* Label */}
        {label && (
          <span className="text-sm font-mono text-muted-foreground min-w-[60px] truncate">
            {label}
          </span>
        )}

        {/* Type badge with dropdown */}
        {!hideTypeSelector ? (
          <DropdownMenu>
            <DropdownMenuTrigger asChild disabled={disabled}>
              <button type="button" className="shrink-0">
                {renderTypeBadge()}
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start">
              {VALUE_TYPE_OPTIONS.map((option) => {
                const IconComponent = VALUE_TYPE_ICONS[option.icon];
                return (
                  <DropdownMenuItem
                    key={option.value}
                    onClick={() => handleTypeChange(option.value)}
                  >
                    <IconComponent className="h-4 w-4 mr-2" />
                    {option.label}
                  </DropdownMenuItem>
                );
              })}
            </DropdownMenuContent>
          </DropdownMenu>
        ) : (
          renderTypeBadge()
        )}

        {/* Value editor */}
        {renderValueEditor()}

        {/* Error indicator */}
        {directError && (
          <Tooltip>
            <TooltipTrigger asChild>
              <AlertTriangle className="h-4 w-4 text-destructive shrink-0" />
            </TooltipTrigger>
            <TooltipContent>
              <p>{directError.message}</p>
            </TooltipContent>
          </Tooltip>
        )}

        {/* Remove button */}
        {removable && onRemove && !disabled && (
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="h-7 w-7 text-muted-foreground hover:text-destructive shrink-0"
            onClick={onRemove}
          >
            <X className="h-3.5 w-3.5" />
          </Button>
        )}
      </div>

      {/* Nested composite editor */}
      {renderNestedEditor()}

      {/* Variable picker modal */}
      <VariablePickerModal
        open={isPickerOpen}
        onOpenChange={setIsPickerOpen}
        onSelect={handleVariableSelect}
      />
    </div>
  );
}

/**
 * Inline object editor for nested composites (to avoid circular imports)
 */
function CompositeObjectEditorInline({
  value,
  onChange,
  depth = 0,
  validationErrors = [],
  pathPrefix = '',
}: CompositeObjectEditorProps & { pathPrefix?: string }) {
  const [newFieldName, setNewFieldName] = useState('');

  const handleAddField = () => {
    if (!newFieldName.trim()) return;
    onChange({
      ...value,
      [newFieldName.trim()]: { valueType: 'immediate', value: '' },
    });
    setNewFieldName('');
  };

  const handleFieldChange = (fieldName: string, newValue: CompositeValue) => {
    onChange({ ...value, [fieldName]: newValue });
  };

  const handleRemoveField = (fieldName: string) => {
    const { [fieldName]: _removed, ...rest } = value;
    void _removed; // Intentionally unused - destructuring to remove field
    onChange(rest);
  };

  return (
    <div className="space-y-2">
      {Object.entries(value).map(([fieldName, fieldValue]) => (
        <CompositeValueItem
          key={fieldName}
          label={fieldName}
          value={fieldValue}
          onChange={(newValue) => handleFieldChange(fieldName, newValue)}
          onRemove={() => handleRemoveField(fieldName)}
          depth={depth}
          validationErrors={validationErrors}
          pathPrefix={pathPrefix ? `${pathPrefix}.${fieldName}` : fieldName}
        />
      ))}

      {Object.keys(value).length === 0 && (
        <p className="text-sm text-muted-foreground italic py-2">
          No fields. Add one below.
        </p>
      )}

      {/* Add field */}
      <div className="flex items-center gap-2">
        <Input
          type="text"
          value={newFieldName}
          onChange={(e) => setNewFieldName(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && handleAddField()}
          placeholder="Field name"
          className="h-8 text-sm flex-1"
        />
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={handleAddField}
          disabled={!newFieldName.trim()}
        >
          Add Field
        </Button>
      </div>
    </div>
  );
}

/**
 * Inline array editor for nested composites (to avoid circular imports)
 */
function CompositeArrayEditorInline({
  value,
  onChange,
  depth = 0,
  validationErrors = [],
  pathPrefix = '',
}: CompositeArrayEditorProps & { pathPrefix?: string }) {
  const handleAddItem = (
    type:
      | 'immediate'
      | 'reference'
      | 'composite-object'
      | 'composite-array'
      | 'template'
  ) => {
    let newItem: CompositeValue;
    if (type === 'immediate') {
      newItem = { valueType: 'immediate', value: '' };
    } else if (type === 'reference') {
      newItem = { valueType: 'reference', value: '' };
    } else if (type === 'composite-object') {
      newItem = { valueType: 'composite', value: {} };
    } else {
      newItem = { valueType: 'composite', value: [] };
    }
    onChange([...value, newItem]);
  };

  const handleItemChange = (index: number, newValue: CompositeValue) => {
    const newArray = [...value];
    newArray[index] = newValue;
    onChange(newArray);
  };

  const handleRemoveItem = (index: number) => {
    onChange(value.filter((_, i) => i !== index));
  };

  return (
    <div className="space-y-2">
      {value.map((item, index) => (
        <CompositeValueItem
          key={index}
          label={`[${index}]`}
          value={item}
          onChange={(newValue) => handleItemChange(index, newValue)}
          onRemove={() => handleRemoveItem(index)}
          depth={depth}
          validationErrors={validationErrors}
          pathPrefix={pathPrefix ? `${pathPrefix}[${index}]` : `[${index}]`}
        />
      ))}

      {value.length === 0 && (
        <p className="text-sm text-muted-foreground italic py-2">
          No items. Add one below.
        </p>
      )}

      {/* Add item dropdown */}
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="w-full border-dashed"
          >
            Add Item
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start">
          {VALUE_TYPE_OPTIONS.map((option) => {
            const IconComponent = VALUE_TYPE_ICONS[option.icon];
            return (
              <DropdownMenuItem
                key={option.value}
                onClick={() => handleAddItem(option.value)}
              >
                <IconComponent className="h-4 w-4 mr-2" />
                {option.label}
              </DropdownMenuItem>
            );
          })}
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}
