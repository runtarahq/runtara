import { useState } from 'react';
import { Card, CardContent } from '@/shared/components/ui/card';
import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { ChevronDown, Code2, PlusCircle, X } from 'lucide-react';
import { Condition } from '@/generated/RuntaraRuntimeApi';
import { convertConditionArguments } from '@/shared/utils/condition-type-conversion';

// Property definition helper type
interface PropertyDefinition {
  name?: string;
  dataType?: string;
  required?: boolean;
  defaultValue?: any;
}

// --- TYPES & CONSTANTS ---
type Arity = 'UNARY' | 'BINARY' | 'VARIADIC';

interface Operator {
  key: string;
  label: string;
  arity: Arity;
  dataTypes?: string[]; // Applicable data types for this operator
}

const OPERATORS: Operator[] = [
  { key: 'AND', label: 'AND (all conditions must be true)', arity: 'VARIADIC' },
  { key: 'OR', label: 'OR (any condition must be true)', arity: 'VARIADIC' },
  { key: 'NOT', label: 'NOT (invert condition)', arity: 'UNARY' },
  { key: 'EQ', label: 'Equals (=)', arity: 'BINARY' },
  { key: 'NE', label: 'Not Equals (≠)', arity: 'BINARY' },
  {
    key: 'GT',
    label: 'Greater Than (>)',
    arity: 'BINARY',
    dataTypes: ['INTEGER', 'DECIMAL', 'DATE'],
  },
  {
    key: 'GTE',
    label: 'Greater or Equal (≥)',
    arity: 'BINARY',
    dataTypes: ['INTEGER', 'DECIMAL', 'DATE'],
  },
  {
    key: 'LT',
    label: 'Less Than (<)',
    arity: 'BINARY',
    dataTypes: ['INTEGER', 'DECIMAL', 'DATE'],
  },
  {
    key: 'LTE',
    label: 'Less or Equal (≤)',
    arity: 'BINARY',
    dataTypes: ['INTEGER', 'DECIMAL', 'DATE'],
  },
  { key: 'IN', label: 'In List', arity: 'BINARY' },
  { key: 'NOT_IN', label: 'Not In List', arity: 'BINARY' },
  {
    key: 'CONTAINS',
    label: 'Contains',
    arity: 'BINARY',
    dataTypes: ['STRING'],
  },
  { key: 'IS_EMPTY', label: 'Is Empty', arity: 'UNARY' },
  { key: 'IS_NOT_EMPTY', label: 'Is Not Empty', arity: 'UNARY' },
  { key: 'IS_DEFINED', label: 'Is Defined', arity: 'UNARY' },
];

const renderConditionReadable = (condition?: Condition | null): string => {
  if (!condition) return '';

  const { op, arguments: args } = condition;

  const renderArg = (arg: any): string => {
    if (typeof arg === 'string') return arg;
    if (typeof arg === 'number') return String(arg);
    if (typeof arg === 'boolean') return String(arg);
    if (typeof arg === 'object' && arg !== null && 'op' in arg) {
      return `(${renderConditionReadable(arg)})`;
    }
    return String(arg);
  };

  const safeArgs = args || [];

  switch (op) {
    case 'AND':
      return safeArgs.map(renderArg).join(' AND ') || '';
    case 'OR':
      return safeArgs.map(renderArg).join(' OR ') || '';
    case 'NOT':
      return safeArgs[0] ? `NOT ${renderArg(safeArgs[0])}` : 'NOT';
    case 'EQ':
      return `${safeArgs[0] ? renderArg(safeArgs[0]) : ''} = ${safeArgs[1] ? renderArg(safeArgs[1]) : ''}`;
    case 'NE':
      return `${safeArgs[0] ? renderArg(safeArgs[0]) : ''} ≠ ${safeArgs[1] ? renderArg(safeArgs[1]) : ''}`;
    case 'GT':
      return `${safeArgs[0] ? renderArg(safeArgs[0]) : ''} > ${safeArgs[1] ? renderArg(safeArgs[1]) : ''}`;
    case 'GTE':
      return `${safeArgs[0] ? renderArg(safeArgs[0]) : ''} ≥ ${safeArgs[1] ? renderArg(safeArgs[1]) : ''}`;
    case 'LT':
      return `${safeArgs[0] ? renderArg(safeArgs[0]) : ''} < ${safeArgs[1] ? renderArg(safeArgs[1]) : ''}`;
    case 'LTE':
      return `${safeArgs[0] ? renderArg(safeArgs[0]) : ''} ≤ ${safeArgs[1] ? renderArg(safeArgs[1]) : ''}`;
    case 'IN':
      return `${safeArgs[0] ? renderArg(safeArgs[0]) : ''} IN ${safeArgs[1] ? renderArg(safeArgs[1]) : ''}`;
    case 'NOT_IN':
      return `${safeArgs[0] ? renderArg(safeArgs[0]) : ''} NOT IN ${safeArgs[1] ? renderArg(safeArgs[1]) : ''}`;
    case 'CONTAINS':
      return `${safeArgs[0] ? renderArg(safeArgs[0]) : ''} CONTAINS ${safeArgs[1] ? renderArg(safeArgs[1]) : ''}`;
    case 'IS_EMPTY':
      return `${safeArgs[0] ? renderArg(safeArgs[0]) : ''} IS EMPTY`;
    case 'IS_NOT_EMPTY':
      return `${safeArgs[0] ? renderArg(safeArgs[0]) : ''} IS NOT EMPTY`;
    case 'IS_DEFINED':
      return `${safeArgs[0] ? renderArg(safeArgs[0]) : ''} IS DEFINED`;
    default:
      return `${op}(${safeArgs.map(renderArg).join(', ')})`;
  }
};

// --- BUILDER COMPONENT ---
export interface FilterComposerProps {
  value?: Condition | null;
  onChange?: (value: Condition | null) => void;
  schemaDefinition?: Record<string, PropertyDefinition>;
}

export const FilterComposer = ({
  value,
  onChange,
  schemaDefinition = {},
}: FilterComposerProps) => {
  const [condition, setCondition] = useState<Condition | null>(value || null);

  const handleConditionChange = (newCondition: Condition | null) => {
    setCondition(newCondition);
    if (onChange) {
      onChange(newCondition);
    }
  };

  const handleClear = () => {
    handleConditionChange(null);
  };

  return (
    <div className="w-full space-y-2">
      {condition ? (
        <>
          <div className="flex justify-between items-center">
            <div className="text-sm font-medium">Filter Expression</div>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={handleClear}
              className="h-7"
            >
              <X className="h-3 w-3 mr-1" />
              Clear Filter
            </Button>
          </div>
          <ConditionBuilder
            value={condition}
            onChange={handleConditionChange}
            schemaDefinition={schemaDefinition}
          />
          <div className="mt-2 p-2 rounded bg-muted/50 text-xs text-muted-foreground font-mono">
            {renderConditionReadable(condition)}
          </div>
        </>
      ) : (
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() =>
            handleConditionChange({ op: 'EQ', arguments: ['', ''] })
          }
          className="w-full"
        >
          <PlusCircle className="h-4 w-4 mr-2" />
          Add Filter Condition
        </Button>
      )}
    </div>
  );
};

interface ConditionBuilderProps {
  value?: Condition | null;
  onChange?: (condition: Condition | null) => void;
  schemaDefinition?: Record<string, PropertyDefinition>;
  depth?: number;
  canRemove?: boolean;
}

const ConditionBuilder = ({
  value,
  onChange,
  schemaDefinition = {},
  depth = 0,
  canRemove = true,
}: ConditionBuilderProps) => {
  const initialOp = value?.op || 'EQ';
  const initialArgs = value?.arguments || ['', ''];
  const [op, setOp] = useState<string>(initialOp);
  const [args, setArgs] = useState<any[]>(initialArgs);

  const operator = OPERATORS.find((o) => o.key === op) || OPERATORS[0];

  // Get list of fields from schema
  const fieldOptions = Object.entries(schemaDefinition).map(([key, prop]) => ({
    value: key,
    label: prop.name || key,
    dataType: prop.dataType,
  }));

  const updateArgs = (newArgs: any[]) => {
    setArgs(newArgs);
    if (onChange) {
      // Apply type conversion to arguments before passing to parent
      const convertedArgs = convertConditionArguments(
        op,
        newArgs,
        schemaDefinition
      );
      onChange({ op, arguments: convertedArgs });
    }
  };

  const handleArgChange = (index: number, value: any) => {
    const newArgs = [...args];
    newArgs[index] = value;
    updateArgs(newArgs);
  };

  const handleAddArgument = () => {
    // For logical operators (AND/OR/NOT), new arguments should be nested
    // conditions so that each row renders as a full sub-condition with both
    // a field selector and a value input. Plain string arguments only make
    // sense for binary/comparison operators.
    const isLogical = ['NOT', 'AND', 'OR'].includes(op);
    const newArg = isLogical ? { op: 'EQ', arguments: ['', ''] } : '';
    updateArgs([...args, newArg]);
  };

  const handleRemoveArgument = (index: number) => {
    const newArgs = args.filter((_, i) => i !== index);
    updateArgs(newArgs);
  };

  const handleOperatorChange = (value: string) => {
    const newOp = value;
    const newOperator = OPERATORS.find((o) => o.key === newOp) || OPERATORS[0];
    const newArity = newOperator.arity;
    // Logical operators (NOT, AND, OR) need nested conditions, not strings
    const isLogical = ['NOT', 'AND', 'OR'].includes(newOp);
    const emptyArg = isLogical ? { op: 'EQ', arguments: ['', ''] } : '';
    let newArgs: any[];
    if (newArity === 'UNARY') newArgs = [emptyArg];
    else if (newArity === 'BINARY') newArgs = [emptyArg, emptyArg];
    else newArgs = [emptyArg];
    setOp(newOp);
    setArgs(newArgs);
    if (onChange) {
      const convertedArgs = convertConditionArguments(
        newOp,
        newArgs,
        schemaDefinition
      );
      onChange({ op: newOp, arguments: convertedArgs });
    }
  };

  const handleRemoveCondition = () => {
    if (onChange) {
      onChange(null);
    }
  };

  const isCondition = (val: any): val is Condition => {
    return (
      typeof val === 'object' &&
      val !== null &&
      'op' in val &&
      val.op !== undefined
    );
  };

  // Check if the value argument should be a date input
  const selectedFieldName = isCondition(args[0]) ? null : (args[0] as string);
  const selectedFieldType = selectedFieldName
    ? fieldOptions.find((f) => f.value === selectedFieldName)?.dataType
    : null;
  const isDateField = selectedFieldType === 'DATE';

  // Check if the first argument should be a field selector
  const isFieldBasedOperator = [
    'EQ',
    'NE',
    'GT',
    'GTE',
    'LT',
    'LTE',
    'CONTAINS',
    'IS_EMPTY',
    'IS_NOT_EMPTY',
    'IS_DEFINED',
  ].includes(op);

  return (
    <Card className={`my-2 border shadow-sm ${depth > 0 ? 'ml-4' : ''}`}>
      <CardContent className="p-3 space-y-2">
        <div className="flex items-center gap-2">
          <Select value={op} onValueChange={handleOperatorChange}>
            <SelectTrigger className="w-[280px] h-8 text-sm">
              <SelectValue placeholder="Operator" />
              <ChevronDown className="ml-auto h-4 w-4 opacity-50" />
            </SelectTrigger>
            <SelectContent>
              {OPERATORS.map((o) => (
                <SelectItem key={o.key} value={o.key}>
                  {o.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {depth > 0 && canRemove && (
            <Button
              type="button"
              variant="ghost"
              size="icon"
              onClick={handleRemoveCondition}
              className="h-8 w-8 ml-auto"
            >
              <X className="h-4 w-4" />
            </Button>
          )}
        </div>

        {args.map((arg, index) => (
          <div key={index} className="flex items-center gap-2">
            {isCondition(arg) ? (
              <div className="flex-1">
                <ConditionBuilder
                  value={arg}
                  onChange={(nested) => handleArgChange(index, nested)}
                  schemaDefinition={schemaDefinition}
                  depth={depth + 1}
                  canRemove={operator.arity !== 'UNARY'}
                />
              </div>
            ) : (
              <>
                {/* First argument for field-based operators should be a field selector */}
                {index === 0 && isFieldBasedOperator ? (
                  <Select
                    value={arg as string}
                    onValueChange={(value) => handleArgChange(index, value)}
                  >
                    <SelectTrigger className="flex-1 h-8 text-sm">
                      <SelectValue placeholder="Select field..." />
                    </SelectTrigger>
                    <SelectContent>
                      {fieldOptions.map((field) => (
                        <SelectItem key={field.value} value={field.value}>
                          {field.label}
                          {field.dataType && (
                            <span className="ml-2 text-xs text-muted-foreground">
                              ({field.dataType})
                            </span>
                          )}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                ) : (
                  <Input
                    className="flex-1 h-8 text-sm"
                    type={index > 0 && isDateField ? 'datetime-local' : 'text'}
                    value={arg as string}
                    onChange={(e) => handleArgChange(index, e.target.value)}
                    placeholder={
                      index === 0 && !isFieldBasedOperator
                        ? 'Enter condition...'
                        : operator.key === 'IN' || operator.key === 'NOT_IN'
                          ? 'Comma-separated values'
                          : index === 0
                            ? 'Field name'
                            : isDateField
                              ? 'Select date...'
                              : 'Value'
                    }
                  />
                )}
                {operator.arity === 'VARIADIC' && (
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    onClick={() =>
                      handleArgChange(index, {
                        op: 'EQ',
                        arguments: ['', ''],
                      })
                    }
                    className="h-8 w-8"
                    title="Convert to nested condition"
                  >
                    <Code2 className="h-4 w-4" />
                  </Button>
                )}
                {operator.arity === 'VARIADIC' && args.length > 1 && (
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    onClick={() => handleRemoveArgument(index)}
                    className="h-8 w-8"
                  >
                    <X className="h-4 w-4" />
                  </Button>
                )}
              </>
            )}
          </div>
        ))}

        {operator.arity === 'VARIADIC' && (
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={handleAddArgument}
            className="text-xs w-full"
          >
            <PlusCircle className="h-4 w-4 mr-1" /> Add Condition
          </Button>
        )}
      </CardContent>
    </Card>
  );
};
