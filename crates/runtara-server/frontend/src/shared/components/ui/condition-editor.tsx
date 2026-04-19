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

// Argument with value type metadata (for reference vs immediate values)
export interface ConditionArgument {
  valueType: 'immediate' | 'reference';
  value: string;
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
      return 'bg-slate-100 text-slate-600 dark:bg-slate-800 dark:text-slate-400';
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
                className={`h-6 w-6 rounded border border-current flex items-center justify-center text-[9px] font-bold shrink-0 transition-colors hover:opacity-80 ${getArgumentValueTypeColor(
                  value
                )}`}
              >
                {getArgumentValueTypeSymbol(value)}
              </button>
            </DropdownMenuTrigger>
          </TooltipTrigger>
          <TooltipContent className="border-0">
            <p className="font-semibold text-foreground text-xs">
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
                  className={`cursor-pointer h-10 px-2 rounded-md focus:bg-accent/50 hover:bg-accent/40 transition-colors ${
                    isSelected ? 'bg-accent/60 ring-1 ring-primary/30' : ''
                  }`}
                >
                  <div className="flex items-center gap-2 w-full">
                    <span
                      className={`h-5 w-5 rounded border flex items-center justify-center text-[8px] font-bold shrink-0 ${getArgumentValueTypeColor(
                        option.value
                      )} ${isSelected ? 'ring-1 ring-primary' : 'border-current'}`}
                    >
                      {getArgumentValueTypeSymbol(option.value)}
                    </span>
                    <div className="flex flex-col flex-1 min-w-0">
                      <span
                        className={`font-medium text-xs leading-tight ${isSelected ? 'text-primary' : ''}`}
                      >
                        {option.label}
                        {isSelected && ' ✓'}
                      </span>
                      <span className="text-[10px] text-muted-foreground leading-tight truncate">
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

// Helper to get the display value from an argument
const getArgumentDisplayValue = (
  arg: Condition | string | ConditionArgument
): string => {
  if (typeof arg === 'string') return arg;
  if (isConditionArgument(arg)) return arg.value;
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
    const value = typeof arg === 'string' ? arg : arg.value;
    if (value === 'true' || value === 'false') return 'boolean';
    if (!isNaN(Number(value)) && value !== '') return 'number';
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

// Autocomplete suggestion interface
interface VariableSuggestion {
  label: string;
  value: string;
  description?: string;
  group: 'Workflow Inputs' | 'Step Outputs' | 'Current Item' | 'Loop Context';
  type?: string;
  stepName?: string; // Step name for display
  stepId?: string; // Step ID for reference
}

// Helper functions for autocomplete
function composeVariableSuggestions(
  previousSteps: any[],
  isInsideWhileLoop?: boolean
): VariableSuggestion[] {
  const suggestions: VariableSuggestion[] = [];

  // Add hardcoded workflow input suggestions
  suggestions.push({
    label: 'workflow.inputs.data',
    value: 'workflow.inputs.data',
    description: 'Workflow input data',
    group: 'Workflow Inputs',
  });

  suggestions.push({
    label: 'workflow.inputs.variables',
    value: 'workflow.inputs.variables',
    description: 'Workflow input variables',
    group: 'Workflow Inputs',
  });

  // Add current item references (used in Filter/Split step conditions)
  suggestions.push({
    label: 'item',
    value: 'item',
    description: 'Current array item',
    group: 'Current Item',
  });
  const commonItemFields = [
    'id',
    'name',
    'title',
    'status',
    'type',
    'value',
    'key',
    'email',
    'price',
    'quantity',
    'created_at',
    'updated_at',
  ];
  for (const field of commonItemFields) {
    suggestions.push({
      label: `item.${field}`,
      value: `item.${field}`,
      description: `Current item ${field}`,
      group: 'Current Item',
    });
  }

  // Add loop context references when inside a While loop
  if (isInsideWhileLoop) {
    suggestions.push({
      label: 'loop.index',
      value: 'loop.index',
      description: 'Current iteration counter (0-based)',
      group: 'Loop Context',
    });
    suggestions.push({
      label: 'loop.outputs',
      value: 'loop.outputs',
      description:
        'Finish step outputs from previous iteration (null on first)',
      group: 'Loop Context',
    });
  }

  // Add suggestions from previous steps' outputs
  for (const step of previousSteps) {
    if (step.outputs && Array.isArray(step.outputs)) {
      const flattenParams = (params: any[], prefix = ''): any[] => {
        const result: any[] = [];
        for (const param of params) {
          result.push({
            path: param.path,
            type: param.type,
            name: param.name,
          });
          if (param.children && param.children.length > 0) {
            result.push(...flattenParams(param.children, prefix));
          }
        }
        return result;
      };

      const flattenedParams = flattenParams(step.outputs);
      for (const param of flattenedParams) {
        // Extract field path from full path (e.g., "steps['id'].outputs.field" -> "field")
        const outputsPrefix = `steps['${step.id}'].outputs`;
        let fieldPath = param.path;
        if (param.path.startsWith(outputsPrefix)) {
          fieldPath = param.path.slice(outputsPrefix.length);
          // Remove leading dot if present
          if (fieldPath.startsWith('.')) {
            fieldPath = fieldPath.slice(1);
          }
        }

        suggestions.push({
          label: fieldPath || 'outputs', // Show just the field path, not full path
          value: param.path, // Keep full path as value for actual use
          description: step.name || 'Step output', // Step name as description
          group: 'Step Outputs',
          type: param.type,
          stepName: step.name, // Add step name for display
          stepId: step.id, // Add step ID for reference
        });
      }
    }
  }

  return suggestions;
}

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

function groupSuggestions(
  suggestions: VariableSuggestion[]
): Record<string, VariableSuggestion[]> {
  const grouped: Record<string, VariableSuggestion[]> = {
    'Loop Context': [],
    'Current Item': [],
    'Workflow Inputs': [],
    'Step Outputs': [],
  };
  for (const suggestion of suggestions) {
    grouped[suggestion.group].push(suggestion);
  }
  return grouped;
}

// Variable Picker Modal for reference selection
const ConditionVariablePickerModal = ({
  open,
  onOpenChange,
  onSelect,
  previousSteps,
  isInsideWhileLoop = false,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSelect: (variable: VariableSuggestion) => void;
  previousSteps: any[];
  isInsideWhileLoop?: boolean;
}) => {
  const [searchQuery, setSearchQuery] = useState('');

  const allSuggestions = useMemo(
    () => composeVariableSuggestions(previousSteps, isInsideWhileLoop),
    [previousSteps, isInsideWhileLoop]
  );

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
            <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
            <Input
              placeholder="Search variables..."
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              className="pl-9 font-mono text-sm"
              autoFocus
            />
          </div>

          {/* Variable list */}
          <div className="max-h-[400px] overflow-y-auto space-y-4">
            {filteredSuggestions.length === 0 ? (
              <div className="text-center py-8 text-muted-foreground">
                <Inbox className="h-8 w-8 mx-auto mb-2 opacity-50" />
                <p>No variables found</p>
              </div>
            ) : (
              <>
                {/* Workflow Inputs */}
                {groupedSuggestions['Workflow Inputs'].length > 0 && (
                  <div>
                    <h4 className="text-xs font-semibold text-muted-foreground uppercase tracking-wide mb-2">
                      Workflow Inputs
                    </h4>
                    <div className="space-y-0.5">
                      {groupedSuggestions['Workflow Inputs'].map(
                        (suggestion) => (
                          <button
                            key={suggestion.value}
                            type="button"
                            onClick={() => handleSelect(suggestion)}
                            className="w-full flex items-center gap-2 px-2 py-1.5 rounded hover:bg-accent text-left transition-colors text-muted-foreground hover:text-foreground"
                          >
                            <div className="flex-1 min-w-0">
                              <p className="font-mono text-sm truncate">
                                {suggestion.label}
                              </p>
                              {suggestion.description && (
                                <p className="text-xs truncate opacity-70">
                                  {suggestion.description}
                                </p>
                              )}
                            </div>
                            {suggestion.type && (
                              <span className="text-[11px] font-mono px-1.5 py-0.5 rounded shrink-0 text-muted-foreground bg-black/5 dark:bg-white/10">
                                {suggestion.type}
                              </span>
                            )}
                          </button>
                        )
                      )}
                    </div>
                  </div>
                )}

                {/* Current Item (Filter/Split context) */}
                {groupedSuggestions['Current Item'].length > 0 && (
                  <div>
                    <h4 className="text-xs font-semibold text-muted-foreground uppercase tracking-wide mb-2">
                      Current Item
                    </h4>
                    <div className="space-y-0.5">
                      {groupedSuggestions['Current Item'].map((suggestion) => (
                        <button
                          key={suggestion.value}
                          type="button"
                          onClick={() => handleSelect(suggestion)}
                          className="w-full flex items-center gap-2 px-2 py-1.5 rounded hover:bg-accent text-left transition-colors text-muted-foreground hover:text-foreground overflow-hidden"
                        >
                          <div className="flex-1 min-w-0">
                            <p className="text-sm font-mono truncate">
                              {suggestion.label}
                            </p>
                          </div>
                        </button>
                      ))}
                    </div>
                  </div>
                )}

                {/* Step Outputs */}
                {groupedSuggestions['Step Outputs'].length > 0 && (
                  <div>
                    <h4 className="text-xs font-semibold text-muted-foreground uppercase tracking-wide mb-2">
                      Step Outputs
                    </h4>
                    <div className="space-y-0.5">
                      {groupedSuggestions['Step Outputs'].map((suggestion) => (
                        <button
                          key={suggestion.value}
                          type="button"
                          onClick={() => handleSelect(suggestion)}
                          className="w-full flex items-center gap-2 px-2 py-1.5 rounded hover:bg-accent text-left transition-colors text-muted-foreground hover:text-foreground overflow-hidden"
                        >
                          <div className="flex-1 min-w-0">
                            <p className="text-sm truncate">
                              <span className="font-medium">
                                {suggestion.stepName || suggestion.description}
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
                          </div>
                          {suggestion.type && (
                            <span className="text-[11px] font-mono px-1.5 py-0.5 rounded shrink-0 text-muted-foreground bg-black/5 dark:bg-white/10">
                              {suggestion.type}
                            </span>
                          )}
                        </button>
                      ))}
                    </div>
                  </div>
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
  previousSteps: any[]
): string => {
  // Check if it's a step output reference
  const stepMatch = value.match(/steps\['([^']+)'\]\.outputs\.?(.*)?/);
  if (stepMatch) {
    const stepId = stepMatch[1];
    const fieldPath = stepMatch[2] || 'outputs';
    const step = previousSteps.find((s) => s.id === stepId);
    const stepName = step?.name || `Step ${stepId.slice(0, 8)}...`;
    return fieldPath ? `${stepName} → ${fieldPath}` : stepName;
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
  previousSteps = [],
}: {
  value: string;
  onRemove: () => void;
  onClick: () => void;
  disabled?: boolean;
  previousSteps?: any[];
}) => {
  const displayValue = formatReferenceForDisplay(value, previousSteps);

  return (
    <span className="inline-flex items-center gap-1.5 px-2 py-1 text-xs bg-emerald-50 border border-emerald-200 rounded text-emerald-700 dark:bg-emerald-950 dark:border-emerald-800 dark:text-emerald-300">
      <button
        type="button"
        onClick={onClick}
        disabled={disabled}
        className="truncate hover:underline max-w-[200px]"
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
      <div className="flex items-center gap-1.5 flex-1">
        <Select
          value={value || 'true'}
          onValueChange={onChange}
          disabled={disabled}
        >
          <SelectTrigger className="h-7 text-xs flex-1">
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
          <SelectTrigger className="h-7 text-xs w-20">
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
    <div className="flex items-center gap-1.5 flex-1">
      <Input
        type={immediateType === 'number' ? 'number' : 'text'}
        className="h-7 text-xs flex-1"
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
        <SelectTrigger className="h-7 text-xs w-20">
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
  previousSteps?: any[]; // StepInfo[] - for autocomplete suggestions
  isInsideWhileLoop?: boolean; // Show loop.* references
}

export const ConditionEditor = ({
  value,
  onChange,
  disabled = false,
  previousSteps = [],
  isInsideWhileLoop = false,
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
        previousSteps={previousSteps}
        isInsideWhileLoop={isInsideWhileLoop}
      />
      {/* Expression preview */}
      {readableExpression && (
        <div className="mt-3 px-2 py-1.5 bg-slate-100 dark:bg-slate-800 rounded text-[11px] font-mono text-slate-600 dark:text-slate-400 break-words">
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
  previousSteps = [],
  isInsideWhileLoop = false,
  inlineControls,
}: {
  value?: Condition;
  onChange?: (condition: Condition) => void;
  disabled?: boolean;
  previousSteps?: any[];
  isInsideWhileLoop?: boolean;
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
    <div
      className={
        isNested
          ? 'pl-3 border-l-2 border-gray-200 dark:border-gray-700 ml-1'
          : ''
      }
    >
      {/* Compact operator select with optional inline controls */}
      <div className="flex items-center gap-1">
        <Select
          value={op}
          onValueChange={handleOperatorChange}
          disabled={disabled}
        >
          <SelectTrigger className="w-auto min-w-[80px] h-7 text-xs font-semibold px-2 border-gray-300 dark:border-gray-600">
            <SelectValue placeholder="Op" />
          </SelectTrigger>
          <SelectContent>
            <div className="text-[10px] font-semibold text-muted-foreground px-2 py-1">
              Logic
            </div>
            {OPERATORS.filter((o) => ['AND', 'OR', 'NOT'].includes(o.key)).map(
              (o) => (
                <SelectItem key={o.key} value={o.key} className="text-xs">
                  {o.key}
                </SelectItem>
              )
            )}
            <div className="text-[10px] font-semibold text-muted-foreground px-2 py-1 mt-1">
              Compare
            </div>
            {OPERATORS.filter((o) =>
              ['EQ', 'NE', 'GT', 'GTE', 'LT', 'LTE'].includes(o.key)
            ).map((o) => (
              <SelectItem key={o.key} value={o.key} className="text-xs">
                {o.key} ({o.label})
              </SelectItem>
            ))}
            <div className="text-[10px] font-semibold text-muted-foreground px-2 py-1 mt-1">
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
            <div className="text-[10px] font-semibold text-muted-foreground px-2 py-1 mt-1">
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
      <div className="flex flex-col gap-1.5 mt-2">
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
                  previousSteps={previousSteps}
                  isInsideWhileLoop={isInsideWhileLoop}
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
                <div className="flex-1 flex items-center gap-1.5">
                  {displayValue ? (
                    <ReferencePill
                      value={displayValue}
                      onRemove={() => handleRemoveReference(index)}
                      onClick={() => setPickerOpenForIndex(index)}
                      disabled={disabled}
                      previousSteps={previousSteps}
                    />
                  ) : (
                    <button
                      type="button"
                      onClick={() => setPickerOpenForIndex(index)}
                      disabled={disabled}
                      className="flex items-center h-7 px-2 text-xs text-muted-foreground border border-dashed border-gray-300 dark:border-gray-600 rounded hover:bg-muted/50 hover:border-gray-400 transition-colors disabled:opacity-50"
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
                <div className="flex-1 flex items-center gap-1.5">
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
            className="self-start px-2 py-1 text-[11px] text-muted-foreground border border-dashed border-gray-300 dark:border-gray-600 rounded hover:bg-muted/50 hover:border-gray-400 transition-colors"
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
        previousSteps={previousSteps}
        isInsideWhileLoop={isInsideWhileLoop}
      />
    </div>
  );
};
