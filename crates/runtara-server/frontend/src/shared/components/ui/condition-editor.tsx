/* eslint-disable react-refresh/only-export-components */
// Exports condition types/enums with the component
import { useState, useRef, useEffect, useMemo } from 'react';
import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from '@/shared/components/ui/dialog';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/shared/components/ui/dropdown-menu';
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/shared/components/ui/tooltip';
import { Search, Inbox, X, Trash2 } from 'lucide-react';
import { convertConditionArguments } from '@/shared/utils/condition-type-conversion';

// --- TYPES & CONSTANTS ---
type Arity = 'UNARY' | 'BINARY' | 'VARIADIC';

interface Operator {
  key: string;
  label: string;
  arity: Arity;
}

const OPERATORS: Operator[] = [
  { key: 'AND', label: 'Logical AND', arity: 'VARIADIC' },
  { key: 'OR', label: 'Logical OR', arity: 'VARIADIC' },
  { key: 'NOT', label: 'Logical NOT', arity: 'UNARY' },
  { key: 'EQ', label: 'Equals', arity: 'BINARY' },
  { key: 'NE', label: 'Not Equals', arity: 'BINARY' },
  { key: 'GT', label: 'Greater Than', arity: 'BINARY' },
  { key: 'GTE', label: 'Greater or Equal', arity: 'BINARY' },
  { key: 'LT', label: 'Less Than', arity: 'BINARY' },
  { key: 'LTE', label: 'Less or Equal', arity: 'BINARY' },
  { key: 'IN', label: 'In List', arity: 'BINARY' },
  { key: 'NOT_IN', label: 'Not In List', arity: 'BINARY' },
  { key: 'STARTS_WITH', label: 'Starts With', arity: 'BINARY' },
  { key: 'ENDS_WITH', label: 'Ends With', arity: 'BINARY' },
  { key: 'CONTAINS', label: 'Contains', arity: 'BINARY' },
  { key: 'IS_EMPTY', label: 'Is Empty', arity: 'UNARY' },
  { key: 'IS_NOT_EMPTY', label: 'Is Not Empty', arity: 'UNARY' },
  { key: 'IS_DEFINED', label: 'Is Defined', arity: 'UNARY' },
  { key: 'LENGTH', label: 'Length', arity: 'UNARY' },
];

export interface Condition {
  type: 'operation';
  op: string;
  arguments: (Condition | string | ConditionArgument)[];
}

// Immediate value types for type selection
type ImmediateValueType = 'string' | 'number' | 'boolean';

const IMMEDIATE_TYPE_OPTIONS: { value: ImmediateValueType; label: string }[] = [
  { value: 'string', label: 'String' },
  { value: 'number', label: 'Number' },
  { value: 'boolean', label: 'Boolean' },
];

// Argument with value type metadata (for reference vs immediate values).
// `value` is typed JSON, not just string: stored definitions round-trip
// booleans, numbers, and arrays (IN/NOT_IN lists) verbatim.
export interface ConditionArgument {
  valueType: 'immediate' | 'reference';
  value: any;
  immediateType?: ImmediateValueType; // Type hint for immediate values
}

// Type for selecting argument value type (immediate, reference, or operation)
type ArgumentValueType = 'immediate' | 'reference' | 'operation';

interface ArgumentValueTypeOption {
  value: ArgumentValueType;
  label: string;
  description: string;
}

const ARGUMENT_VALUE_TYPE_OPTIONS: ArgumentValueTypeOption[] = [
  {
    value: 'immediate',
    label: 'Immediate',
    description: 'Literal value (string, number, boolean)',
  },
  {
    value: 'reference',
    label: 'Reference',
    description: 'Reference to data path (e.g., steps.step1.outputs.result)',
  },
  {
    value: 'operation',
    label: 'Operation',
    description: 'Nested condition expression',
  },
];

// Get color class based on argument value type
const getArgumentValueTypeColor = (type: ArgumentValueType): string => {
  switch (type) {
    case 'reference':
      return 'bg-cyan-100 text-cyan-700 dark:bg-cyan-950 dark:text-cyan-300';
    case 'immediate':
      return 'bg-orange-100 text-orange-700 dark:bg-orange-950 dark:text-orange-300';
    case 'operation':
      return 'bg-violet-100 text-violet-700 dark:bg-violet-950 dark:text-violet-300';
    default:
      return 'bg-muted text-muted-foreground';
  }
};

// Get icon/symbol representation
const getArgumentValueTypeSymbol = (type: ArgumentValueType): string => {
  switch (type) {
    case 'reference':
      return '{}';
    case 'immediate':
      return '=';
    case 'operation':
      return '</>';
    default:
      return '?';
  }
};

// Selector component for argument value type - compact version
const ArgumentValueTypeSelector = ({
  value = 'immediate',
  onChange,
  disabled = false,
}: {
  value?: ArgumentValueType;
  onChange: (value: ArgumentValueType) => void;
  disabled?: boolean;
}) => {
  const selectedOption =
    ARGUMENT_VALUE_TYPE_OPTIONS.find((opt) => opt.value === value) ||
    ARGUMENT_VALUE_TYPE_OPTIONS[0];

  return (
    <TooltipProvider>
      <Tooltip>
        <DropdownMenu>
          <TooltipTrigger asChild>
            <DropdownMenuTrigger asChild>
              <button
                type="button"
                disabled={disabled}
                className={`flex h-6 w-6 shrink-0 items-center justify-center rounded border border-current text-[9px] font-bold transition-colors hover:opacity-80 ${getArgumentValueTypeColor(
                  value
                )}`}
              >
                {getArgumentValueTypeSymbol(value)}
              </button>
            </DropdownMenuTrigger>
          </TooltipTrigger>
          <TooltipContent className="border-0">
            <p className="text-xs font-semibold text-foreground">
              {selectedOption.label}
            </p>
            <p className="text-[10px] opacity-80">
              {selectedOption.description}
            </p>
          </TooltipContent>
          <DropdownMenuContent align="end" className="w-56 p-1">
            {ARGUMENT_VALUE_TYPE_OPTIONS.map((option) => {
              const isSelected = option.value === value;
              return (
                <DropdownMenuItem
                  key={option.value}
                  onClick={() => onChange(option.value)}
                  className={`h-10 cursor-pointer rounded-md px-2 transition-colors hover:bg-accent/40 focus:bg-accent/50 ${
                    isSelected ? 'bg-accent/60 ring-1 ring-primary/30' : ''
                  }`}
                >
                  <div className="flex w-full items-center gap-2">
                    <span
                      className={`flex h-5 w-5 shrink-0 items-center justify-center rounded border text-[8px] font-bold ${getArgumentValueTypeColor(
                        option.value
                      )} ${isSelected ? 'ring-1 ring-primary' : 'border-current'}`}
                    >
                      {getArgumentValueTypeSymbol(option.value)}
                    </span>
                    <div className="flex min-w-0 flex-1 flex-col">
                      <span
                        className={`text-xs font-medium leading-tight ${isSelected ? 'text-primary' : ''}`}
                      >
                        {option.label}
                        {isSelected && ' ✓'}
                      </span>
                      <span className="truncate text-[10px] leading-tight text-muted-foreground">
                        {option.description}
                      </span>
                    </div>
                  </div>
                </DropdownMenuItem>
              );
            })}
          </DropdownMenuContent>
        </DropdownMenu>
      </Tooltip>
    </TooltipProvider>
  );
};

// Helper to check if an argument is a ConditionArgument with valueType
const isConditionArgument = (arg: any): arg is ConditionArgument => {
  return (
    typeof arg === 'object' &&
    arg !== null &&
    'valueType' in arg &&
    'value' in arg &&
    !('op' in arg)
  );
};

// Helper to get the display value from an argument. Stored definitions carry
// typed JSON immediates (boolean, number, array) — render them as text.
const getArgumentDisplayValue = (
  arg: Condition | string | ConditionArgument
): string => {
  if (typeof arg === 'string') return arg;
  if (isConditionArgument(arg)) {
    const value = arg.value as unknown;
    if (typeof value === 'string') return value;
    if (Array.isArray(value)) return value.join(', ');
    if (value === null || value === undefined) return '';
    return String(value);
  }
  return ''; // For Condition, handled separately
};

// Helper to get the immediate type from an argument
const getArgumentImmediateType = (
  arg: Condition | string | ConditionArgument
): ImmediateValueType => {
  if (isConditionArgument(arg) && arg.immediateType) {
    return arg.immediateType;
  }
  // Try to infer type from value
  if (typeof arg === 'string' || isConditionArgument(arg)) {
    const value = typeof arg === 'string' ? arg : (arg.value as unknown);
    if (typeof value === 'boolean') return 'boolean';
    if (typeof value === 'number') return 'number';
    if (value === 'true' || value === 'false') return 'boolean';
    if (typeof value === 'string' && value !== '' && !isNaN(Number(value))) {
      return 'number';
    }
  }
  return 'string';
};

// Helper to get the value type from an argument
const getArgumentValueType = (
  arg: Condition | string | ConditionArgument
): ArgumentValueType => {
  if (typeof arg === 'object' && arg !== null && 'op' in arg)
    return 'operation';
  if (isConditionArgument(arg)) return arg.valueType;
  return 'immediate'; // Default for plain strings
};

/**
 * Autocomplete suggestion for condition references. Structurally compatible
 * with the canonical VariableSuggestion from
 * features/workflows .../InputMappingValueField/VariableSuggestions — the
 * editor no longer composes suggestions itself (its old forked composer
 * carried hardcoded guessed item.* field names not driven by any schema);
 * call sites compose via composeConditionSuggestions and pass them in.
 */
export interface ConditionSuggestion {
  label: string;
  value: string;
  description?: string;
  group: string;
  type?: string;
  stepName?: string; // Step name for display
  stepId?: string; // Step ID for reference
}

type VariableSuggestion = ConditionSuggestion;

function filterSuggestions(
  suggestions: VariableSuggestion[],
  query: string
): VariableSuggestion[] {
  if (!query) {
    return suggestions;
  }
  const lowerQuery = query.toLowerCase();
  return suggestions.filter((suggestion) => {
    const lowerLabel = suggestion.label.toLowerCase();
    const lowerDescription = suggestion.description?.toLowerCase() || '';
    return (
      lowerLabel.includes(lowerQuery) || lowerDescription.includes(lowerQuery)
    );
  });
}

/** Preferred display order; unknown groups append in insertion order. */
const GROUP_ORDER = [
  'Current Item',
  'Loop Context',
  'Split Scope',
  'Wait Scope',
  'Workflow Inputs',
  'Variables',
  'Step Outputs',
];

function groupSuggestions(
  suggestions: VariableSuggestion[]
): Map<string, VariableSuggestion[]> {
  const grouped = new Map<string, VariableSuggestion[]>();
  for (const group of GROUP_ORDER) {
    grouped.set(group, []);
  }
  for (const suggestion of suggestions) {
    const bucket = grouped.get(suggestion.group);
    if (bucket) {
      bucket.push(suggestion);
    } else {
      grouped.set(suggestion.group, [suggestion]);
    }
  }
  return grouped;
}

// Variable Picker Modal for reference selection
const ConditionVariablePickerModal = ({
  open,
  onOpenChange,
  onSelect,
  suggestions,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSelect: (variable: VariableSuggestion) => void;
  suggestions: VariableSuggestion[];
}) => {
  const [searchQuery, setSearchQuery] = useState('');

  const allSuggestions = suggestions;

  const filteredSuggestions = useMemo(
    () => filterSuggestions(allSuggestions, searchQuery),
    [allSuggestions, searchQuery]
  );

  const groupedSuggestions = useMemo(
    () => groupSuggestions(filteredSuggestions),
    [filteredSuggestions]
  );

  const handleSelect = (suggestion: VariableSuggestion) => {
    onSelect(suggestion);
    onOpenChange(false);
    setSearchQuery('');
  };

  const handleOpenChange = (newOpen: boolean) => {
    onOpenChange(newOpen);
    if (!newOpen) {
      setSearchQuery('');
    }
  };

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="sm:max-w-[500px]">
        <DialogHeader>
          <DialogTitle>Select Variable</DialogTitle>
          <DialogDescription>
            Choose a variable from workflow inputs or previous step outputs
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4">
          {/* Search input */}
          <div className="relative">
            <Search className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
            <Input
              placeholder="Search variables..."
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              className="pl-9 font-mono text-sm"
              autoFocus
            />
          </div>

          {/* Variable list */}
          <div className="max-h-[400px] space-y-4 overflow-y-auto">
            {/* Free-text path entry: any legal reference path can be used
                even when it is not in the suggestion list */}
            {searchQuery.trim() !== '' &&
              !allSuggestions.some(
                (suggestion) => suggestion.value === searchQuery.trim()
              ) && (
                <button
                  type="button"
                  onClick={() =>
                    handleSelect({
                      label: searchQuery.trim(),
                      value: searchQuery.trim(),
                      group: 'Workflow Inputs',
                    })
                  }
                  className="flex w-full items-center gap-2 rounded border border-dashed px-2 py-1.5 text-left text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                >
                  <div className="min-w-0 flex-1">
                    <p className="truncate font-mono text-sm">
                      {searchQuery.trim()}
                    </p>
                    <p className="truncate text-xs opacity-70">
                      Use as custom reference path
                    </p>
                  </div>
                </button>
              )}
            {filteredSuggestions.length === 0 ? (
              <div className="py-8 text-center text-muted-foreground">
                <Inbox className="mx-auto mb-2 h-8 w-8 opacity-50" />
                <p>No matching variables</p>
              </div>
            ) : (
              <>
                {[...groupedSuggestions.entries()].map(
                  ([group, groupSuggestionsList]) =>
                    groupSuggestionsList.length > 0 && (
                      <div key={group}>
                        <h4 className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                          {group}
                        </h4>
                        <div className="space-y-0.5">
                          {groupSuggestionsList.map((suggestion) => (
                            <button
                              key={suggestion.value}
                              type="button"
                              onClick={() => handleSelect(suggestion)}
                              className="flex w-full items-center gap-2 overflow-hidden rounded px-2 py-1.5 text-left text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                            >
                              <div className="min-w-0 flex-1">
                                {suggestion.stepName ? (
                                  <p className="truncate text-sm">
                                    <span className="font-medium">
                                      {suggestion.stepName}
                                    </span>
                                    {suggestion.label && (
                                      <span className="text-muted-foreground">
                                        {' → '}
                                        <span className="font-mono">
                                          {suggestion.label}
                                        </span>
                                      </span>
                                    )}
                                  </p>
                                ) : (
                                  <p className="truncate font-mono text-sm">
                                    {suggestion.label}
                                  </p>
                                )}
                                {suggestion.description && (
                                  <p className="truncate text-xs opacity-70">
                                    {suggestion.description}
                                  </p>
                                )}
                              </div>
                              {suggestion.type && (
                                <span className="shrink-0 rounded bg-black/5 px-1.5 py-0.5 font-mono text-[11px] text-muted-foreground dark:bg-white/10">
                                  {suggestion.type}
                                </span>
                              )}
                            </button>
                          ))}
                        </div>
                      </div>
                    )
                )}
              </>
            )}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
};

// Helper to format reference value for display
// Converts "steps['uuid'].outputs.field" to "StepName → field"
const formatReferenceForDisplay = (
  value: string,
  suggestions: VariableSuggestion[]
): string => {
  // A suggestion the user could have picked carries the friendly parts.
  const suggestion = suggestions.find((s) => s.value === value);
  if (suggestion) {
    return suggestion.stepName
      ? `${suggestion.stepName} → ${suggestion.label}`
      : suggestion.label;
  }
  // Fallback for hand-typed paths: steps['id'].outputs.field and sibling
  // fields directly under steps['id'] (e.g. hasFailures, route).
  const stepMatch = value.match(/steps\['([^']+)'\]\.?(.*)$/);
  if (stepMatch) {
    const stepId = stepMatch[1];
    let fieldPath = stepMatch[2] || 'outputs';
    if (fieldPath.startsWith('outputs.')) {
      fieldPath = fieldPath.slice('outputs.'.length);
    }
    const stepName =
      suggestions.find((s) => s.stepId === stepId)?.stepName ||
      `Step ${stepId.slice(0, 8)}...`;
    return `${stepName} → ${fieldPath}`;
  }
  // For workflow inputs, just return the value as-is
  return value;
};

// Reference pill component to display selected reference - compact green pill style
const ReferencePill = ({
  value,
  onRemove,
  onClick,
  disabled,
  suggestions = [],
}: {
  value: string;
  onRemove: () => void;
  onClick: () => void;
  disabled?: boolean;
  suggestions?: VariableSuggestion[];
}) => {
  const displayValue = formatReferenceForDisplay(value, suggestions);

  return (
    <span className="inline-flex items-center gap-1.5 rounded border border-emerald-200 bg-emerald-50 px-2 py-1 text-xs text-emerald-700 dark:border-emerald-800 dark:bg-emerald-950 dark:text-emerald-300">
      <button
        type="button"
        onClick={onClick}
        disabled={disabled}
        className="max-w-[200px] truncate hover:underline"
        title={value}
      >
        {displayValue}
      </button>
      {!disabled && (
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            onRemove();
          }}
          className="text-emerald-400 hover:text-emerald-600 dark:hover:text-emerald-200"
        >
          <X className="h-3 w-3" />
        </button>
      )}
    </span>
  );
};

// Immediate value input component with type selector - compact version
const ImmediateValueInput = ({
  value,
  onChange,
  immediateType,
  onImmediateTypeChange,
  placeholder,
  disabled,
}: {
  value: string;
  onChange: (value: string) => void;
  immediateType: ImmediateValueType;
  onImmediateTypeChange: (type: ImmediateValueType) => void;
  placeholder?: string;
  disabled?: boolean;
}) => {
  // Render appropriate input based on immediate type
  if (immediateType === 'boolean') {
    return (
      <div className="flex flex-1 items-center gap-1.5">
        <Select
          value={value || 'true'}
          onValueChange={onChange}
          disabled={disabled}
        >
          <SelectTrigger className="h-7 flex-1 text-xs">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="true" className="text-xs">
              true
            </SelectItem>
            <SelectItem value="false" className="text-xs">
              false
            </SelectItem>
          </SelectContent>
        </Select>
        <Select
          value={immediateType}
          onValueChange={(val) =>
            onImmediateTypeChange(val as ImmediateValueType)
          }
          disabled={disabled}
        >
          <SelectTrigger className="h-7 w-20 text-xs">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {IMMEDIATE_TYPE_OPTIONS.map((opt) => (
              <SelectItem key={opt.value} value={opt.value} className="text-xs">
                {opt.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
    );
  }

  return (
    <div className="flex flex-1 items-center gap-1.5">
      <Input
        type={immediateType === 'number' ? 'number' : 'text'}
        className="h-7 flex-1 text-xs"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        disabled={disabled}
      />
      <Select
        value={immediateType}
        onValueChange={(val) =>
          onImmediateTypeChange(val as ImmediateValueType)
        }
        disabled={disabled}
      >
        <SelectTrigger className="h-7 w-20 text-xs">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {IMMEDIATE_TYPE_OPTIONS.map((opt) => (
            <SelectItem key={opt.value} value={opt.value} className="text-xs">
              {opt.label}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );
};

export const renderConditionReadable = (condition?: Condition): string => {
  // Handle undefined condition
  if (!condition) {
    return '';
  }

  const { op, arguments: args } = condition;

  const renderArg = (arg: string | Condition | ConditionArgument): string => {
    if (typeof arg === 'string') {
      return arg;
    }

    // Handle ConditionArgument with valueType
    if (isConditionArgument(arg)) {
      return arg.value;
    }

    // Handle condition with undefined op
    if (!arg.op) {
      return '';
    }

    return `(${renderConditionReadable(arg)})`;
  };

  switch (op) {
    case 'AND':
      return args?.map(renderArg)?.join(' AND ') || '';
    case 'OR':
      return args?.map(renderArg)?.join(' OR ') || '';
    case 'NOT':
      return args && args[0] ? `NOT ${renderArg(args[0])}` : 'NOT';
    case 'EQ':
      return `${args && args[0] ? renderArg(args[0]) : ''} = ${args && args[1] ? renderArg(args[1]) : ''}`;
    case 'NE':
      return `${args && args[0] ? renderArg(args[0]) : ''} != ${args && args[1] ? renderArg(args[1]) : ''}`;
    case 'GT':
      return `${args && args[0] ? renderArg(args[0]) : ''} > ${args && args[1] ? renderArg(args[1]) : ''}`;
    case 'GTE':
      return `${args && args[0] ? renderArg(args[0]) : ''} >= ${args && args[1] ? renderArg(args[1]) : ''}`;
    case 'LT':
      return `${args && args[0] ? renderArg(args[0]) : ''} < ${args && args[1] ? renderArg(args[1]) : ''}`;
    case 'LTE':
      return `${args && args[0] ? renderArg(args[0]) : ''} <= ${args && args[1] ? renderArg(args[1]) : ''}`;
    case 'IN':
      return `${args && args[0] ? renderArg(args[0]) : ''} IN ${args && args[1] ? renderArg(args[1]) : ''}`;
    case 'NOT_IN':
      return `${args && args[0] ? renderArg(args[0]) : ''} NOT IN ${args && args[1] ? renderArg(args[1]) : ''}`;
    case 'STARTS_WITH':
      return `${args && args[0] ? renderArg(args[0]) : ''} STARTS WITH ${args && args[1] ? renderArg(args[1]) : ''}`;
    case 'ENDS_WITH':
      return `${args && args[0] ? renderArg(args[0]) : ''} ENDS WITH ${args && args[1] ? renderArg(args[1]) : ''}`;
    case 'CONTAINS':
      return `${args && args[0] ? renderArg(args[0]) : ''} CONTAINS ${args && args[1] ? renderArg(args[1]) : ''}`;
    case 'IS_EMPTY':
      return `${args && args[0] ? renderArg(args[0]) : ''} IS EMPTY`;
    case 'IS_NOT_EMPTY':
      return `${args && args[0] ? renderArg(args[0]) : ''} IS NOT EMPTY`;
    case 'IS_DEFINED':
      return `${args && args[0] ? renderArg(args[0]) : ''} IS DEFINED`;
    case 'LENGTH':
      return `LENGTH(${args && args[0] ? renderArg(args[0]) : ''})`;
    default:
      // Handle undefined op
      if (!op) {
        return '';
      }
      return `${op}(${args?.map(renderArg)?.join(', ') || ''})`;
  }
};

// --- BUILDER COMPONENT ---
interface ConditionEditorProps {
  value?: string;
  onChange?: (value: string) => void;
  disabled?: boolean;
  /**
   * Reference suggestions for the picker — compose with
   * composeConditionSuggestions (features/workflows VariableSuggestions) so
   * every surface shares the one canonical, schema-driven pipeline.
   */
  suggestions?: ConditionSuggestion[];
}

export const ConditionEditor = ({
  value,
  onChange,
  disabled = false,
  suggestions = [],
}: ConditionEditorProps) => {
  // Parse condition value from string
  const parseConditionValue = (val?: string): Condition | undefined => {
    if (!val) return undefined;
    try {
      const parsed = JSON.parse(val);
      // Validate that the parsed object has the required properties
      if (
        parsed &&
        typeof parsed === 'object' &&
        'op' in parsed &&
        parsed.op !== undefined &&
        'arguments' in parsed
      ) {
        return parsed as Condition;
      } else {
        console.error('Invalid condition format:', parsed);
        return undefined;
      }
    } catch (e) {
      console.error('Failed to parse condition value:', e);
      return undefined;
    }
  };

  const [condition, setCondition] = useState<Condition | undefined>(
    parseConditionValue(value)
  );

  // Track the last value we synced from props to avoid unnecessary updates
  const lastSyncedValue = useRef<string | undefined>(value);

  // Update condition when value prop changes (e.g., when form data loads)
  // Only update if the string value actually changed
  useEffect(() => {
    if (value !== lastSyncedValue.current) {
      const parsed = parseConditionValue(value);
      if (parsed) {
        lastSyncedValue.current = value;
        setCondition(parsed);
      }
    }
  }, [value]);

  const handleConditionChange = (newCondition: Condition) => {
    setCondition(newCondition);
    if (onChange) {
      const jsonValue = JSON.stringify(newCondition);
      // Update the ref so we don't re-parse this value when it comes back from parent
      lastSyncedValue.current = jsonValue;
      onChange(jsonValue);
    }
  };

  const readableExpression = condition
    ? renderConditionReadable(condition)
    : '';

  return (
    <div className="w-full">
      <ConditionBuilder
        value={condition}
        onChange={handleConditionChange}
        disabled={disabled}
        suggestions={suggestions}
      />
      {/* Expression preview */}
      {readableExpression && (
        <div className="mt-3 break-words rounded bg-muted px-2 py-1.5 font-mono text-[11px] text-muted-foreground">
          {readableExpression}
        </div>
      )}
    </div>
  );
};

const ConditionBuilder = ({
  value,
  onChange,
  disabled = false,
  suggestions = [],
  inlineControls,
}: {
  value?: Condition;
  onChange?: (condition: Condition) => void;
  disabled?: boolean;
  suggestions?: ConditionSuggestion[];
  inlineControls?: React.ReactNode;
}) => {
  const initialOp = value?.op || 'EQ';
  const initialArgs = value?.arguments || ['', ''];
  const [op, setOp] = useState<string>(initialOp);
  const [args, setArgs] =
    useState<(string | Condition | ConditionArgument)[]>(initialArgs);

  // State for variable picker modal - track which argument index is being edited
  const [pickerOpenForIndex, setPickerOpenForIndex] = useState<number | null>(
    null
  );

  // Track the last synced value to avoid unnecessary state updates
  const lastSyncedValueRef = useRef<string | null>(null);

  // Update state when value prop changes (e.g., when form data loads)
  // Use JSON comparison to only sync when actual content changes, not just object reference
  useEffect(() => {
    if (value) {
      const valueStr = JSON.stringify(value);
      // Only update if the value actually changed from what we last synced
      if (lastSyncedValueRef.current !== valueStr) {
        lastSyncedValueRef.current = valueStr;
        setOp(value.op);
        setArgs(value.arguments);
      }
    }
  }, [value]);

  // Find the operator or default to the first one if not found
  const operator = OPERATORS.find((o) => o.key === op) || OPERATORS[0];

  const updateArgs = (newArgs: (string | Condition | ConditionArgument)[]) => {
    setArgs(newArgs);
    if (onChange) {
      // Apply type conversion to arguments before passing to parent
      const convertedArgs = convertConditionArguments(op, newArgs);
      const newCondition: Condition = {
        type: 'operation',
        op,
        arguments: convertedArgs,
      };
      // Update the ref to prevent the useEffect from overwriting user changes
      lastSyncedValueRef.current = JSON.stringify(newCondition);
      onChange(newCondition);
    }
  };

  const handleArgChange = (
    index: number,
    value: string | Condition | ConditionArgument
  ) => {
    const newArgs = [...args];
    newArgs[index] = value;
    updateArgs(newArgs);
  };

  const handleArgValueChange = (index: number, newValue: string) => {
    const currentArg = args[index];
    const currentValueType = getArgumentValueType(currentArg);
    const currentImmediateType = getArgumentImmediateType(currentArg);

    // If it's a reference type, wrap the value in a ConditionArgument
    if (currentValueType === 'reference') {
      handleArgChange(index, { valueType: 'reference', value: newValue });
    } else {
      // For immediate, preserve the immediate type
      handleArgChange(index, {
        valueType: 'immediate',
        value: newValue,
        immediateType: currentImmediateType,
      });
    }
  };

  const handleImmediateTypeChange = (
    index: number,
    newImmediateType: ImmediateValueType
  ) => {
    const currentArg = args[index];
    const currentValue = getArgumentDisplayValue(currentArg);

    // Convert value if needed when changing type
    let convertedValue = currentValue;
    if (newImmediateType === 'boolean') {
      convertedValue =
        currentValue === 'true' || currentValue === '1' ? 'true' : 'false';
    } else if (newImmediateType === 'number') {
      const num = parseFloat(currentValue);
      convertedValue = isNaN(num) ? '' : String(num);
    }

    handleArgChange(index, {
      valueType: 'immediate',
      value: convertedValue,
      immediateType: newImmediateType,
    });
  };

  const handleValueTypeChange = (
    index: number,
    newValueType: ArgumentValueType
  ) => {
    const currentArg = args[index];
    const currentValue = getArgumentDisplayValue(currentArg);

    if (newValueType === 'operation') {
      // Convert to nested condition
      handleArgChange(index, {
        type: 'operation',
        op: 'EQ',
        arguments: ['', ''],
      });
    } else if (newValueType === 'reference') {
      // Convert to reference - open the picker modal
      setPickerOpenForIndex(index);
    } else {
      // Convert to immediate with default string type
      handleArgChange(index, {
        valueType: 'immediate',
        value: currentValue,
        immediateType: 'string',
      });
    }
  };

  const handleVariableSelect = (
    index: number,
    variable: VariableSuggestion
  ) => {
    handleArgChange(index, { valueType: 'reference', value: variable.value });
    setPickerOpenForIndex(null);
  };

  const handleRemoveReference = (index: number) => {
    // When removing reference, switch to immediate mode
    handleArgChange(index, {
      valueType: 'immediate',
      value: '',
      immediateType: 'string',
    });
  };

  const handleAddArgument = () => {
    updateArgs([
      ...args,
      { valueType: 'immediate', value: '', immediateType: 'string' },
    ]);
  };

  const handleRemoveArgument = (index: number) => {
    const newArgs = args.filter((_, i) => i !== index);
    updateArgs(newArgs);
  };

  const handleOperatorChange = (value: string) => {
    const newOp = value;
    // Find the operator or default to the first one if not found
    const newOperator = OPERATORS.find((o) => o.key === newOp) || OPERATORS[0];
    const newArity = newOperator.arity;
    let newArgs: (string | Condition | ConditionArgument)[];
    if (newArity === 'UNARY')
      newArgs = [
        { valueType: 'immediate', value: '', immediateType: 'string' },
      ];
    else if (newArity === 'BINARY')
      newArgs = [
        { valueType: 'immediate', value: '', immediateType: 'string' },
        { valueType: 'immediate', value: '', immediateType: 'string' },
      ];
    else
      newArgs = [
        { valueType: 'immediate', value: '', immediateType: 'string' },
      ];
    setOp(newOp);
    setArgs(newArgs);
    if (onChange) {
      const convertedArgs = convertConditionArguments(newOp, newArgs);
      const newCondition: Condition = {
        type: 'operation',
        op: newOp,
        arguments: convertedArgs,
      };
      // Update the ref to prevent the useEffect from overwriting user changes
      lastSyncedValueRef.current = JSON.stringify(newCondition);
      onChange(newCondition);
    }
  };

  const isCondition = (
    val: string | Condition | ConditionArgument
  ): val is Condition => {
    return (
      typeof val === 'object' &&
      val !== null &&
      'op' in val &&
      val.op !== undefined
    );
  };

  // Check if this is a variadic operator (can add/remove arguments)
  const isVariadicOperator = operator.arity === 'VARIADIC';

  // Handler to convert an operation argument back to immediate value
  const handleClearOperationArgument = (index: number) => {
    handleArgChange(index, {
      valueType: 'immediate',
      value: '',
      immediateType: 'string',
    });
  };

  // Determine if this is the root level or nested
  const isNested = value !== undefined;

  return (
    <div className={isNested ? 'ml-1 border-l-2 border-border pl-3' : ''}>
      {/* Compact operator select with optional inline controls */}
      <div className="flex items-center gap-1">
        <Select
          value={op}
          onValueChange={handleOperatorChange}
          disabled={disabled}
        >
          <SelectTrigger className="h-7 w-auto min-w-[80px] border-input px-2 text-xs font-semibold">
            <SelectValue placeholder="Op" />
          </SelectTrigger>
          <SelectContent>
            <div className="px-2 py-1 text-[10px] font-semibold text-muted-foreground">
              Logic
            </div>
            {OPERATORS.filter((o) => ['AND', 'OR', 'NOT'].includes(o.key)).map(
              (o) => (
                <SelectItem key={o.key} value={o.key} className="text-xs">
                  {o.key}
                </SelectItem>
              )
            )}
            <div className="mt-1 px-2 py-1 text-[10px] font-semibold text-muted-foreground">
              Compare
            </div>
            {OPERATORS.filter((o) =>
              ['EQ', 'NE', 'GT', 'GTE', 'LT', 'LTE'].includes(o.key)
            ).map((o) => (
              <SelectItem key={o.key} value={o.key} className="text-xs">
                {o.key} ({o.label})
              </SelectItem>
            ))}
            <div className="mt-1 px-2 py-1 text-[10px] font-semibold text-muted-foreground">
              Check
            </div>
            {OPERATORS.filter((o) =>
              ['IS_EMPTY', 'IS_NOT_EMPTY', 'IS_DEFINED', 'LENGTH'].includes(
                o.key
              )
            ).map((o) => (
              <SelectItem key={o.key} value={o.key} className="text-xs">
                {o.label}
              </SelectItem>
            ))}
            <div className="mt-1 px-2 py-1 text-[10px] font-semibold text-muted-foreground">
              List/String
            </div>
            {OPERATORS.filter((o) =>
              ['IN', 'NOT_IN', 'CONTAINS'].includes(o.key)
            ).map((o) => (
              <SelectItem key={o.key} value={o.key} className="text-xs">
                {o.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {inlineControls}
      </div>

      {/* Arguments with tighter spacing */}
      <div className="mt-2 flex flex-col gap-1.5">
        {args.map((arg, index) => {
          const currentValueType = getArgumentValueType(arg);
          const displayValue = getArgumentDisplayValue(arg);
          const immediateType = getArgumentImmediateType(arg);

          return (
            <div key={index} className="flex items-start gap-1.5">
              {isCondition(arg) ? (
                // Nested operation - render with inline controls
                <ConditionBuilder
                  value={arg}
                  onChange={(nested) => handleArgChange(index, nested)}
                  disabled={disabled}
                  suggestions={suggestions}
                  inlineControls={
                    <>
                      <ArgumentValueTypeSelector
                        value="operation"
                        onChange={(newType) =>
                          handleValueTypeChange(index, newType)
                        }
                        disabled={disabled}
                      />
                      {isVariadicOperator && !disabled ? (
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon"
                          className="h-6 w-6 text-muted-foreground hover:text-destructive"
                          onClick={() => handleRemoveArgument(index)}
                        >
                          <Trash2 className="h-3 w-3" />
                        </Button>
                      ) : (
                        !disabled && (
                          <TooltipProvider>
                            <Tooltip>
                              <TooltipTrigger asChild>
                                <Button
                                  type="button"
                                  variant="ghost"
                                  size="icon"
                                  className="h-6 w-6 text-muted-foreground hover:text-destructive"
                                  onClick={() =>
                                    handleClearOperationArgument(index)
                                  }
                                >
                                  <Trash2 className="h-3 w-3" />
                                </Button>
                              </TooltipTrigger>
                              <TooltipContent>
                                <p>Clear nested condition</p>
                              </TooltipContent>
                            </Tooltip>
                          </TooltipProvider>
                        )
                      )}
                    </>
                  }
                />
              ) : currentValueType === 'reference' ? (
                // Reference mode - show compact pill
                <div className="flex flex-1 items-center gap-1.5">
                  {displayValue ? (
                    <ReferencePill
                      value={displayValue}
                      onRemove={() => handleRemoveReference(index)}
                      onClick={() => setPickerOpenForIndex(index)}
                      disabled={disabled}
                      suggestions={suggestions}
                    />
                  ) : (
                    <button
                      type="button"
                      onClick={() => setPickerOpenForIndex(index)}
                      disabled={disabled}
                      className="flex h-7 items-center rounded border border-dashed border-input px-2 text-xs text-muted-foreground transition-colors hover:border-muted-foreground/50 hover:bg-muted/50 disabled:opacity-50"
                    >
                      Select variable...
                    </button>
                  )}
                  <ArgumentValueTypeSelector
                    value={currentValueType}
                    onChange={(newType) =>
                      handleValueTypeChange(index, newType)
                    }
                    disabled={disabled}
                  />
                  {isVariadicOperator && !disabled && (
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      className="h-6 w-6 text-muted-foreground hover:text-destructive"
                      onClick={() => handleRemoveArgument(index)}
                    >
                      <Trash2 className="h-3 w-3" />
                    </Button>
                  )}
                </div>
              ) : (
                // Immediate mode - compact input
                <div className="flex flex-1 items-center gap-1.5">
                  <ImmediateValueInput
                    value={displayValue}
                    onChange={(value) => handleArgValueChange(index, value)}
                    immediateType={immediateType}
                    onImmediateTypeChange={(type) =>
                      handleImmediateTypeChange(index, type)
                    }
                    placeholder={`Arg ${index + 1}`}
                    disabled={disabled}
                  />
                  <ArgumentValueTypeSelector
                    value={currentValueType}
                    onChange={(newType) =>
                      handleValueTypeChange(index, newType)
                    }
                    disabled={disabled}
                  />
                  {isVariadicOperator && !disabled && (
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      className="h-6 w-6 text-muted-foreground hover:text-destructive"
                      onClick={() => handleRemoveArgument(index)}
                    >
                      <Trash2 className="h-3 w-3" />
                    </Button>
                  )}
                </div>
              )}
            </div>
          );
        })}

        {/* Compact "+ Add" button with dashed border */}
        {operator.arity === 'VARIADIC' && !disabled && (
          <button
            type="button"
            onClick={(e) => {
              e.preventDefault();
              e.stopPropagation();
              handleAddArgument();
            }}
            className="self-start rounded border border-dashed border-input px-2 py-1 text-[11px] text-muted-foreground transition-colors hover:border-muted-foreground/50 hover:bg-muted/50"
            disabled={disabled}
          >
            + Add
          </button>
        )}
      </div>

      {/* Variable Picker Modal */}
      <ConditionVariablePickerModal
        open={pickerOpenForIndex !== null}
        onOpenChange={(open) => {
          if (!open) setPickerOpenForIndex(null);
        }}
        onSelect={(variable) => {
          if (pickerOpenForIndex !== null) {
            handleVariableSelect(pickerOpenForIndex, variable);
          }
        }}
        suggestions={suggestions}
      />
    </div>
  );
};
