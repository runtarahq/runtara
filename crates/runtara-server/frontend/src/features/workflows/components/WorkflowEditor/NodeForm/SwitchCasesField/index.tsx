import { useController, useFormContext, useWatch } from 'react-hook-form';
import { Icons } from '@/shared/components/icons.tsx';
import { Button } from '@/shared/components/ui/button.tsx';
import { Input } from '@/shared/components/ui/input.tsx';
import { SourceMappingValueField } from '../SourceMappingValueField';
import { Textarea } from '@/shared/components/ui/textarea.tsx';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/shared/components/ui/dialog.tsx';
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/shared/components/ui/alert-dialog.tsx';
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectSeparator,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select.tsx';
import { Switch as ToggleSwitch } from '@/shared/components/ui/switch.tsx';
import { Label } from '@/shared/components/ui/label.tsx';
import { useContext, useEffect, useState } from 'react';
import { NodeFormContext } from '../NodeFormContext';
import { ValueType } from '../TypeHintSelector';

type MatchType =
  | 'exact'
  | 'ne'
  | 'in'
  | 'not_in'
  | 'gt'
  | 'gte'
  | 'lt'
  | 'lte'
  | 'between'
  | 'range'
  | 'starts_with'
  | 'ends_with'
  | 'contains'
  | 'is_defined'
  | 'is_empty'
  | 'is_not_empty';

// Grouped match types for the dropdown
const MATCH_TYPE_GROUPS: {
  label: string;
  items: { value: MatchType; label: string; description: string }[];
}[] = [
  {
    label: 'Equality',
    items: [
      { value: 'exact', label: '=', description: 'Equals' },
      { value: 'ne', label: '≠', description: 'Not equals' },
    ],
  },
  {
    label: 'Comparison',
    items: [
      { value: 'gt', label: '>', description: 'Greater than' },
      { value: 'gte', label: '≥', description: 'Greater or equal' },
      { value: 'lt', label: '<', description: 'Less than' },
      { value: 'lte', label: '≤', description: 'Less or equal' },
    ],
  },
  {
    label: 'Range',
    items: [
      {
        value: 'between',
        label: 'Between',
        description: 'Inclusive range [min, max]',
      },
      {
        value: 'range',
        label: 'Range',
        description: 'Custom range with operators',
      },
    ],
  },
  {
    label: 'Collection',
    items: [
      { value: 'in', label: 'In', description: 'Value in list' },
      { value: 'not_in', label: 'Not In', description: 'Value not in list' },
    ],
  },
  {
    label: 'String',
    items: [
      {
        value: 'starts_with',
        label: 'Starts With',
        description: 'Prefix match',
      },
      { value: 'ends_with', label: 'Ends With', description: 'Suffix match' },
      {
        value: 'contains',
        label: 'Contains',
        description: 'Substring match',
      },
    ],
  },
  {
    label: 'Existence',
    items: [
      {
        value: 'is_defined',
        label: 'Is Defined',
        description: 'Field exists',
      },
      { value: 'is_empty', label: 'Is Empty', description: 'Null or empty' },
      {
        value: 'is_not_empty',
        label: 'Not Empty',
        description: 'Has a value',
      },
    ],
  },
];

// Existence operators that don't need a match value
const EXISTENCE_OPERATORS = new Set<MatchType>([
  'is_defined',
  'is_empty',
  'is_not_empty',
]);

export function SwitchCasesField(props: any) {
  const { label, name } = props;
  const { setValue } = useFormContext();
  const { nodeId } = useContext(NodeFormContext);
  const [showRoutingConfirm, setShowRoutingConfirm] = useState(false);
  const [editDialogOpen, setEditDialogOpen] = useState(false);
  const [editingField, setEditingField] = useState<{
    type: 'defaultOutput' | 'caseMatch' | 'caseOutput';
    index?: number;
    currentValue: string;
  } | null>(null);
  const [editingValue, setEditingValue] = useState('');

  const {
    fieldState: { error },
  } = useController({ name });

  const watchFieldArray = useWatch({ name, defaultValue: [] });

  // Ensure watchFieldArray is an array
  const fieldArray = Array.isArray(watchFieldArray) ? watchFieldArray : [];

  // Extract switch configuration from inputMapping array
  // Format: [{ type: 'value', value: '{{expr}}' }, { type: 'cases', value: [...] }, { type: 'default', value: {...} }, { type: 'routingMode', value: boolean }]
  const valueField = fieldArray.find((item: any) => item.type === 'value');
  const casesField = fieldArray.find((item: any) => item.type === 'cases');
  const defaultField = fieldArray.find((item: any) => item.type === 'default');
  const routingModeField = fieldArray.find(
    (item: any) => item.type === 'routingMode'
  );

  const switchValueTypeHint = (valueField?.typeHint as ValueType) || 'string';
  const casesArray = Array.isArray(casesField?.value) ? casesField.value : [];
  // The default output is optional: an absent entry means "no match fails the
  // step" at runtime, so its mere presence is meaningful.
  const hasDefaultOutput = defaultField !== undefined;
  const defaultOutput = defaultField?.value;
  const isRoutingMode = routingModeField?.value === true;

  // Initialize the inputMapping structure if empty (only for new nodes, not when editing)
  useEffect(() => {
    // Only initialize if we're creating a new node (no nodeId) and the array
    // is empty. Deliberately no 'default' entry: a default output must be
    // authored explicitly (absent default = no-match is an error).
    if (!nodeId && fieldArray.length === 0) {
      setValue(name, [
        { type: 'value', value: '', typeHint: 'auto', valueType: 'reference' },
        { type: 'cases', value: [], typeHint: 'json' },
      ]);
    }
  }, [fieldArray.length, name, setValue, nodeId]);

  // Convert string value to typed value based on valueType
  const convertToType = (value: string, valueType: ValueType): any => {
    if (!value || valueType === 'string') {
      return value;
    }

    try {
      switch (valueType) {
        case 'integer':
          return parseInt(value, 10);
        case 'number':
          return parseFloat(value);
        case 'boolean':
          return value.toLowerCase() === 'true';
        case 'json':
          return JSON.parse(value);
        default:
          return value;
      }
    } catch {
      return value; // Return original if conversion fails
    }
  };

  const updateCases = (newCases: any[]) => {
    const newArray = [...fieldArray];
    const casesIndex = newArray.findIndex((item: any) => item.type === 'cases');
    if (casesIndex >= 0) {
      newArray[casesIndex] = { ...newArray[casesIndex], value: newCases };
    } else {
      newArray.push({
        type: 'cases',
        value: newCases,
        typeHint: 'json',
      });
    }
    setValue(name, newArray);
  };

  const updateDefaultOutput = (newDefault: any) => {
    const newArray = [...fieldArray];
    const defaultIndex = newArray.findIndex(
      (item: any) => item.type === 'default'
    );
    if (defaultIndex >= 0) {
      newArray[defaultIndex] = { ...newArray[defaultIndex], value: newDefault };
    } else {
      newArray.push({ type: 'default', value: newDefault, typeHint: 'json' });
    }
    setValue(name, newArray);
  };

  // Remove the default output entry entirely so the saved config carries no
  // `default` key — at runtime an absent default makes a no-match an error.
  const removeDefaultOutput = () => {
    setValue(
      name,
      fieldArray.filter((item: any) => item.type !== 'default')
    );
  };

  const updateRoutingMode = (enabled: boolean) => {
    const newArray = [...fieldArray];
    const idx = newArray.findIndex((item: any) => item.type === 'routingMode');
    if (idx >= 0) {
      newArray[idx] = { ...newArray[idx], value: enabled };
    } else {
      newArray.push({
        type: 'routingMode',
        value: enabled,
        typeHint: 'boolean',
      });
    }

    // When disabling routing, strip route fields from all cases
    if (!enabled) {
      const casesIdx = newArray.findIndex((item: any) => item.type === 'cases');
      if (casesIdx >= 0 && Array.isArray(newArray[casesIdx].value)) {
        newArray[casesIdx] = {
          ...newArray[casesIdx],
          value: newArray[casesIdx].value.map((caseItem: any) => {
            const rest = { ...caseItem };
            delete rest.route;
            return rest;
          }),
        };
      }
    }

    setValue(name, newArray);
  };

  const addCase = () => {
    const newCase: any = { matchType: 'exact', match: '', output: {} };
    if (isRoutingMode) {
      newCase.route = `case_${casesArray.length + 1}`;
    }
    const newCases = [...casesArray, newCase];
    updateCases(newCases);
  };

  const removeCase = (index: number) => {
    const newCases = casesArray.filter((_: any, i: number) => i !== index);
    updateCases(newCases);
  };

  const updateCaseMatchType = (index: number, matchType: MatchType) => {
    const newCases = [...casesArray];
    // Reset match value when changing matchType to avoid invalid data
    let defaultMatch: any;
    if (EXISTENCE_OPERATORS.has(matchType)) {
      defaultMatch = null;
    } else {
      switch (matchType) {
        case 'in':
        case 'not_in':
        case 'between':
          defaultMatch = [];
          break;
        case 'range':
          defaultMatch = {};
          break;
        default:
          defaultMatch = '';
      }
    }
    newCases[index] = { ...newCases[index], matchType, match: defaultMatch };
    updateCases(newCases);
  };

  const updateCaseMatch = (index: number, match: string | string[] | any) => {
    const newCases = [...casesArray];
    const caseItem = newCases[index];
    const matchType = caseItem?.matchType || 'exact';

    // Apply type conversion based on switchValueTypeHint and matchType
    let typedMatch: any;

    if (EXISTENCE_OPERATORS.has(matchType as MatchType)) {
      typedMatch = null;
    } else if (matchType === 'range') {
      // Range expects an object like {gte: 100, lt: 500}
      typedMatch = match;
    } else if (
      matchType === 'in' ||
      matchType === 'not_in' ||
      matchType === 'between'
    ) {
      // 'in', 'not_in' and 'between' expect arrays
      if (Array.isArray(match)) {
        typedMatch = match.map((val) =>
          convertToType(val, switchValueTypeHint)
        );
      } else {
        typedMatch = match;
      }
    } else {
      // exact, ne, gt, gte, lt, lte, starts_with, ends_with, contains expect single values
      if (Array.isArray(match)) {
        typedMatch = match.map((val) =>
          convertToType(val, switchValueTypeHint)
        );
      } else {
        typedMatch = convertToType(match, switchValueTypeHint);
      }
    }

    newCases[index] = { ...newCases[index], match: typedMatch };
    updateCases(newCases);
  };

  const updateCaseOutput = (index: number, output: any) => {
    const newCases = [...casesArray];
    newCases[index] = { ...newCases[index], output };
    updateCases(newCases);
  };

  const updateCaseRoute = (index: number, route: string) => {
    const newCases = [...casesArray];
    newCases[index] = { ...newCases[index], route };
    updateCases(newCases);
  };

  const openEditDialog = (
    type: 'defaultOutput' | 'caseMatch' | 'caseOutput',
    currentValue: string,
    index?: number
  ) => {
    setEditingField({ type, index, currentValue });
    setEditingValue(currentValue);
    setEditDialogOpen(true);
  };

  const handleDialogSave = () => {
    if (!editingField) return;

    if (editingField.type === 'defaultOutput') {
      try {
        const parsed = JSON.parse(editingValue);
        updateDefaultOutput(parsed);
      } catch {
        updateDefaultOutput(editingValue);
      }
    } else if (
      editingField.type === 'caseMatch' &&
      editingField.index !== undefined
    ) {
      const caseItem = casesArray[editingField.index];
      const matchType = caseItem?.matchType || 'exact';
      const parsed = parseMatchValue(editingValue, matchType);
      // updateCaseMatch already handles type conversion
      updateCaseMatch(editingField.index, parsed);
    } else if (
      editingField.type === 'caseOutput' &&
      editingField.index !== undefined
    ) {
      try {
        const parsed = JSON.parse(editingValue);
        updateCaseOutput(editingField.index, parsed);
      } catch {
        updateCaseOutput(editingField.index, editingValue);
      }
    }

    setEditDialogOpen(false);
    setEditingField(null);
    setEditingValue('');
  };

  // Helper to format match value for display based on matchType
  const formatMatchValue = (match: any, matchType: MatchType): string => {
    if (EXISTENCE_OPERATORS.has(matchType)) {
      return '';
    }

    if (matchType === 'range') {
      // Range is an object like {gte: 100, lt: 500} or {min: 1000, max: 5000}
      if (typeof match === 'object' && match !== null) {
        // Handle {min, max} format
        if ('min' in match || 'max' in match) {
          if ('min' in match && 'max' in match) {
            return `${match.min} to ${match.max}`;
          } else if ('min' in match) {
            return `>= ${match.min}`;
          } else if ('max' in match) {
            return `< ${match.max}`;
          }
        }
        // Handle {gte, gt, lt, lte} format
        if (
          'gte' in match ||
          'gt' in match ||
          'lt' in match ||
          'lte' in match
        ) {
          const parts: string[] = [];
          if ('gte' in match) parts.push(`>= ${match.gte}`);
          if ('gt' in match) parts.push(`> ${match.gt}`);
          if ('lt' in match) parts.push(`< ${match.lt}`);
          if ('lte' in match) parts.push(`<= ${match.lte}`);
          return parts.join(' and ');
        }
        // Fallback to JSON
        return JSON.stringify(match);
      }
      return String(match || '');
    } else if (
      matchType === 'in' ||
      matchType === 'not_in' ||
      matchType === 'between'
    ) {
      // Arrays
      if (Array.isArray(match)) {
        return match.join(', ');
      }
      return String(match || '');
    } else {
      // Single values (exact, ne, gt, gte, lt, lte, starts_with, ends_with, contains)
      if (Array.isArray(match)) {
        return match.join(', ');
      }
      return String(match || '');
    }
  };

  // Helper to parse match value from input based on matchType
  const parseMatchValue = (value: string, matchType: MatchType): any => {
    if (EXISTENCE_OPERATORS.has(matchType as MatchType)) {
      return null;
    }

    const trimmed = value.trim();

    if (matchType === 'range') {
      // Try to parse as JSON object
      try {
        return JSON.parse(trimmed);
      } catch {
        return {};
      }
    } else if (
      matchType === 'in' ||
      matchType === 'not_in' ||
      matchType === 'between'
    ) {
      // Parse as array (comma-separated)
      if (trimmed.includes(',')) {
        return trimmed
          .split(',')
          .map((v) => v.trim())
          .filter((v) => v);
      }
      return trimmed ? [trimmed] : [];
    } else {
      // Single value (exact, ne, gt, gte, lt, lte, starts_with, ends_with, contains)
      return trimmed;
    }
  };

  // Get placeholder text for the match input
  const getMatchPlaceholder = (matchType: MatchType): string => {
    switch (matchType) {
      case 'range':
        return '{"gte": 100, "lt": 500}';
      case 'in':
      case 'not_in':
        return 'val1, val2, val3';
      case 'between':
        return 'min, max';
      case 'starts_with':
        return 'prefix';
      case 'ends_with':
        return 'suffix';
      case 'contains':
        return 'substring';
      default:
        return 'value';
    }
  };

  // Get description for the edit dialog based on match type
  const getMatchDescription = (matchType: string): string => {
    switch (matchType) {
      case 'exact':
        return 'Enter a single value for exact match (e.g., "US" or 100)';
      case 'ne':
        return 'Enter a single value for "not equals" comparison';
      case 'in':
        return 'Enter comma-separated values for "in list" matching (e.g., "US, CA, MX")';
      case 'not_in':
        return 'Enter comma-separated values for "not in list" matching';
      case 'gt':
        return 'Enter a single value for "greater than" comparison (e.g., 100)';
      case 'gte':
        return 'Enter a single value for "greater than or equal" comparison';
      case 'lt':
        return 'Enter a single value for "less than" comparison (e.g., 100)';
      case 'lte':
        return 'Enter a single value for "less than or equal" comparison';
      case 'between':
        return 'Enter two comma-separated values for inclusive range (e.g., "100, 500")';
      case 'range':
        return 'Enter a JSON object with operators (e.g., {"gte": 100, "lt": 500})';
      case 'starts_with':
        return 'Enter a prefix string to match against';
      case 'ends_with':
        return 'Enter a suffix string to match against';
      case 'contains':
        return 'Enter a substring to search for';
      default:
        return 'Enter match pattern';
    }
  };

  // Total columns in the cases table (dynamic based on routing mode)
  const totalColumns = isRoutingMode ? 5 : 4;

  return (
    <div>
      <div className="mb-4">{label}</div>

      {/* Value to Switch On — shared source-mapping editor so reference
          type hints, fallback defaults, and composite values round-trip
          through the same path as Split/Filter/GroupBy. */}
      <div className="mb-4">
        <SourceMappingValueField
          name={name}
          label="Value to Switch On"
          description="The value compared against each case. Use reference mode for dynamic values from previous steps."
          suggestions={[]}
          fieldType="any"
          placeholder="Enter value or use reference mode..."
        />
      </div>

      {/* Routing Mode Toggle */}
      <div className="mb-4 flex items-center justify-between border rounded-lg p-3">
        <div className="space-y-0.5">
          <Label className="text-sm font-medium">Routing Mode</Label>
          <div className="text-xs text-muted-foreground">
            Branch execution to different paths by case
          </div>
        </div>
        <ToggleSwitch
          data-testid="switch-routing-mode"
          checked={isRoutingMode}
          onCheckedChange={(checked) => {
            if (!checked && isRoutingMode) {
              // Show confirm dialog when disabling routing mode
              setShowRoutingConfirm(true);
            } else {
              updateRoutingMode(checked);
            }
          }}
        />
      </div>

      {/* Cases */}
      <div className="mb-4">
        <div className="text-sm font-medium text-muted-foreground mb-2">
          Cases
        </div>
        <div className="border rounded-lg">
          <table className="w-full">
            <thead>
              <tr className="border-b">
                <th className="w-32 text-left p-2 text-sm font-medium text-muted-foreground">
                  Match Type
                </th>
                <th className="text-left p-2 text-sm font-medium text-muted-foreground">
                  Match Pattern
                </th>
                <th className="text-left p-2 text-sm font-medium text-muted-foreground">
                  Output
                </th>
                {isRoutingMode && (
                  <th className="w-28 text-left p-2 text-sm font-medium text-muted-foreground">
                    Route
                  </th>
                )}
                <th className="w-16 text-center p-2 text-sm font-medium text-muted-foreground">
                  Actions
                </th>
              </tr>
            </thead>
            <tbody>
              {casesArray.map((caseItem: any, index: number) => {
                const matchType = (caseItem.matchType || 'exact') as MatchType;
                const matchValue = caseItem.match;
                const outputValue = caseItem.output || {};
                const routeValue = caseItem.route || '';
                const isExistence = EXISTENCE_OPERATORS.has(matchType);

                return (
                  <tr key={index} className="border-b hover:bg-muted/30">
                    <td className="p-2">
                      <Select
                        value={matchType}
                        onValueChange={(value) =>
                          updateCaseMatchType(index, value as MatchType)
                        }
                      >
                        <SelectTrigger className="h-7 text-xs border-0 focus:ring-0">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          {MATCH_TYPE_GROUPS.map((group, groupIdx) => (
                            <SelectGroup key={group.label}>
                              {groupIdx > 0 && <SelectSeparator />}
                              <SelectLabel className="text-xs">
                                {group.label}
                              </SelectLabel>
                              {group.items.map((type) => (
                                <SelectItem
                                  key={type.value}
                                  value={type.value}
                                  className="text-xs"
                                >
                                  <div className="flex items-center gap-2">
                                    <span className="font-semibold min-w-[24px]">
                                      {type.label}
                                    </span>
                                    <span className="text-muted-foreground">
                                      {type.description}
                                    </span>
                                  </div>
                                </SelectItem>
                              ))}
                            </SelectGroup>
                          ))}
                        </SelectContent>
                      </Select>
                    </td>
                    <td className="p-2">
                      {isExistence ? (
                        <span className="text-xs text-muted-foreground italic px-1">
                          No value needed
                        </span>
                      ) : (
                        <div className="flex items-center gap-1">
                          <Input
                            data-testid={`switch-case-match-${index}`}
                            defaultValue={formatMatchValue(
                              matchValue,
                              matchType
                            )}
                            onBlur={(e) => {
                              const parsed = parseMatchValue(
                                e.target.value,
                                matchType
                              );
                              updateCaseMatch(index, parsed);
                            }}
                            key={`${index}-${matchType}-${formatMatchValue(matchValue, matchType)}`}
                            className="font-mono text-sm border-0 p-1 h-auto focus-visible:ring-0 focus-visible:ring-offset-0 min-w-0 flex-1"
                            placeholder={getMatchPlaceholder(matchType)}
                          />
                          <Button
                            type="button"
                            variant="ghost"
                            size="icon"
                            className="h-6 w-6 shrink-0"
                            onClick={() =>
                              openEditDialog(
                                'caseMatch',
                                formatMatchValue(matchValue, matchType),
                                index
                              )
                            }
                          >
                            <Icons.edit className="h-3 w-3" />
                          </Button>
                        </div>
                      )}
                    </td>
                    <td className="p-2">
                      <div className="flex items-center gap-1">
                        <Input
                          data-testid={`switch-case-output-${index}`}
                          value={
                            typeof outputValue === 'object'
                              ? JSON.stringify(outputValue)
                              : outputValue
                          }
                          onChange={(e) => {
                            try {
                              const parsed = JSON.parse(e.target.value);
                              updateCaseOutput(index, parsed);
                            } catch {
                              updateCaseOutput(index, e.target.value);
                            }
                          }}
                          className="font-mono text-sm border-0 p-1 h-auto focus-visible:ring-0 focus-visible:ring-offset-0 min-w-0 flex-1"
                          placeholder='{"key": "value"} or simple value'
                        />
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon"
                          className="h-6 w-6 shrink-0"
                          onClick={() =>
                            openEditDialog(
                              'caseOutput',
                              typeof outputValue === 'object'
                                ? JSON.stringify(outputValue, null, 2)
                                : outputValue,
                              index
                            )
                          }
                        >
                          <Icons.edit className="h-3 w-3" />
                        </Button>
                      </div>
                    </td>
                    {isRoutingMode && (
                      <td className="p-2">
                        <Input
                          data-testid={`switch-case-route-${index}`}
                          value={routeValue}
                          onChange={(e) =>
                            updateCaseRoute(index, e.target.value)
                          }
                          className="font-mono text-sm border-0 p-1 h-auto focus-visible:ring-0 focus-visible:ring-offset-0 min-w-0"
                          placeholder={`case_${index + 1}`}
                        />
                      </td>
                    )}
                    <td className="w-16 text-center p-2">
                      <Button
                        type="button"
                        size="icon"
                        variant="ghost"
                        onClick={() => removeCase(index)}
                        className="h-8 w-8"
                      >
                        <Icons.remove className="w-4 h-4" />
                      </Button>
                    </td>
                  </tr>
                );
              })}
              {casesArray.length === 0 && (
                <tr>
                  <td
                    colSpan={totalColumns}
                    className="text-center text-muted-foreground p-4"
                  >
                    No cases added yet. Click "Add Case" to get started.
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
        <div className="mt-2">
          <Button
            data-testid="switch-add-case"
            className="w-full flex gap-1 text-sm"
            type="button"
            size="sm"
            variant="outline"
            onClick={addCase}
          >
            <Icons.add className="w-4 h-4" /> Add Case
          </Button>
        </div>
      </div>

      {/* Default Output — optional. Its presence is semantically meaningful:
          without a default, an unmatched value fails the step at runtime. */}
      <div>
        <div className="text-sm font-medium text-muted-foreground mb-2">
          Default Output
        </div>
        {hasDefaultOutput ? (
          <div className="flex items-center gap-1 border rounded-lg p-2">
            <Input
              data-testid="switch-default-output"
              value={
                typeof defaultOutput === 'object'
                  ? JSON.stringify(defaultOutput)
                  : String(defaultOutput ?? '')
              }
              onChange={(e) => {
                try {
                  const parsed = JSON.parse(e.target.value);
                  updateDefaultOutput(parsed);
                } catch {
                  updateDefaultOutput(e.target.value);
                }
              }}
              className="font-mono text-sm border-0 p-1 h-auto focus-visible:ring-0 focus-visible:ring-offset-0 flex-1"
              placeholder='{"key": "value"} or simple value'
            />
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="h-6 w-6 shrink-0"
              onClick={() =>
                openEditDialog(
                  'defaultOutput',
                  typeof defaultOutput === 'object'
                    ? JSON.stringify(defaultOutput, null, 2)
                    : String(defaultOutput ?? '')
                )
              }
            >
              <Icons.edit className="h-3 w-3" />
            </Button>
            <Button
              data-testid="switch-remove-default"
              type="button"
              variant="ghost"
              size="icon"
              className="h-6 w-6 shrink-0"
              title="Remove default output (unmatched values will fail)"
              onClick={removeDefaultOutput}
            >
              <Icons.remove className="h-3 w-3" />
            </Button>
          </div>
        ) : (
          <div className="flex items-center justify-between gap-2 border rounded-lg p-3">
            <div className="text-xs text-muted-foreground">
              No default — execution fails when no case matches.
            </div>
            <Button
              data-testid="switch-add-default"
              type="button"
              size="sm"
              variant="outline"
              className="shrink-0"
              onClick={() => updateDefaultOutput({})}
            >
              <Icons.add className="w-4 h-4" /> Add Default
            </Button>
          </div>
        )}
      </div>

      {error && (
        <div className="text-[0.8rem] mt-2 font-medium text-destructive">
          {error.message || error.root?.message}
        </div>
      )}

      <Dialog open={editDialogOpen} onOpenChange={setEditDialogOpen}>
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>
              Edit{' '}
              {editingField?.type === 'defaultOutput'
                ? 'Default Output'
                : editingField?.type === 'caseMatch'
                  ? 'Match Pattern'
                  : 'Case Output'}
            </DialogTitle>
            <DialogDescription>
              {editingField?.type === 'caseMatch'
                ? (() => {
                    const caseItem =
                      editingField.index !== undefined
                        ? casesArray[editingField.index]
                        : null;
                    const matchType = caseItem?.matchType || 'exact';
                    return getMatchDescription(matchType);
                  })()
                : 'Enter the output value or JSON object. Moustache templates are not resolved here; for a dynamic value use a {"valueType": "reference", "value": "path.to.value"} object.'}
            </DialogDescription>
          </DialogHeader>
          <div className="py-4">
            <Textarea
              value={editingValue}
              onChange={(e) => setEditingValue(e.target.value)}
              className="font-mono text-sm min-h-[200px]"
              placeholder={
                editingField?.type === 'caseMatch'
                  ? 'Enter match pattern (e.g., US or DE, FR, IT)'
                  : 'Enter value or JSON object'
              }
            />
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setEditDialogOpen(false)}>
              Cancel
            </Button>
            <Button onClick={handleDialogSave}>Save</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Confirm dialog for disabling routing mode */}
      <AlertDialog
        open={showRoutingConfirm}
        onOpenChange={setShowRoutingConfirm}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Disable Routing Mode?</AlertDialogTitle>
            <AlertDialogDescription>
              Disabling routing mode will remove route labels from all cases.
              Any branch connections from this Switch node will need to be
              reconnected. This action cannot be undone.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                updateRoutingMode(false);
                setShowRoutingConfirm(false);
              }}
            >
              Disable Routing
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
