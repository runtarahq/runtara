import { useController, useFormContext, useWatch } from 'react-hook-form';
import { Icons } from '@/shared/components/icons.tsx';
import { Button } from '@/shared/components/ui/button.tsx';
import { Input } from '@/shared/components/ui/input.tsx';
import {
  MappingValueInput,
  type ValueMode,
} from '../InputMappingField/MappingValueInput';
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
import { useContext, useEffect, useRef, useState } from 'react';
import { NodeFormContext } from '../NodeFormContext';
import { ValueType } from '../TypeHintSelector';
import {
  composeVariableSuggestions,
  filterSuggestions,
  groupSuggestions,
  VariableSuggestion,
} from '../InputMappingValueField/VariableSuggestions';

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
  const { setValue, getValues } = useFormContext();
  const { previousSteps, nodeId } = useContext(NodeFormContext);
  const [showRoutingConfirm, setShowRoutingConfirm] = useState(false);
  const [editDialogOpen, setEditDialogOpen] = useState(false);
  const [editingField, setEditingField] = useState<{
    type: 'value' | 'defaultOutput' | 'caseMatch' | 'caseOutput';
    index?: number;
    currentValue: string;
  } | null>(null);
  const [editingValue, setEditingValue] = useState('');

  // Autocomplete state
  const [showAutocomplete, setShowAutocomplete] = useState<boolean>(false);
  const [autocompleteQuery, setAutocompleteQuery] = useState<string>('');
  const [triggerIndex, setTriggerIndex] = useState<number>(-1);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

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

  const switchValue = valueField?.value || '';
  const switchValueType: ValueMode =
    (valueField?.valueType as ValueMode) || 'immediate';
  const switchValueTypeHint =
    (valueField?.typeHint as ValueType) || ValueType.String;
  const casesArray = Array.isArray(casesField?.value) ? casesField.value : [];
  const defaultOutput = defaultField?.value || {};
  const isRoutingMode = routingModeField?.value === true;

  // Initialize the inputMapping structure if empty (only for new nodes, not when editing)
  useEffect(() => {
    // Only initialize if we're creating a new node (no nodeId) and the array is empty
    if (!nodeId && fieldArray.length === 0) {
      setValue(name, [
        { type: 'value', value: '', typeHint: ValueType.String },
        { type: 'cases', value: [], typeHint: ValueType.Json },
        { type: 'default', value: {}, typeHint: ValueType.Json },
      ]);
    }
  }, [fieldArray.length, name, setValue, nodeId]);

  // Convert string value to typed value based on valueType
  const convertToType = (value: string, valueType: ValueType): any => {
    if (!value || valueType === ValueType.String) {
      return value;
    }

    try {
      switch (valueType) {
        case ValueType.Integer:
          return parseInt(value, 10);
        case ValueType.Number:
          return parseFloat(value);
        case ValueType.Boolean:
          return value.toLowerCase() === 'true';
        case ValueType.Json:
          return JSON.parse(value);
        default:
          return value;
      }
    } catch {
      return value; // Return original if conversion fails
    }
  };

  const updateSwitchValue = (newValue: string) => {
    // Read the latest form state (not the stale useWatch closure) so that
    // sequential calls from MappingValueInput (onValueTypeChange then onChange)
    // don't overwrite each other.
    const currentArray = getValues(name) || fieldArray;
    const newArray = [...currentArray];
    const valueIndex = newArray.findIndex((item: any) => item.type === 'value');

    // Get the current value type
    const currentValueType =
      valueIndex >= 0 ? newArray[valueIndex].typeHint : ValueType.String;

    // Convert the value based on value type
    const typedValue = convertToType(newValue, currentValueType);

    if (valueIndex >= 0) {
      newArray[valueIndex] = { ...newArray[valueIndex], value: typedValue };
    } else {
      newArray.push({
        type: 'value',
        value: typedValue,
        typeHint: ValueType.String,
      });
    }
    setValue(name, newArray);
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
        typeHint: ValueType.Json,
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
    type: 'value' | 'defaultOutput' | 'caseMatch' | 'caseOutput',
    currentValue: string,
    index?: number
  ) => {
    setEditingField({ type, index, currentValue });
    setEditingValue(currentValue);
    setEditDialogOpen(true);
  };

  const handleDialogSave = () => {
    if (!editingField) return;

    if (editingField.type === 'value') {
      // updateSwitchValue already handles type conversion
      updateSwitchValue(editingValue);
    } else if (editingField.type === 'defaultOutput') {
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

  // Handle textarea value change for autocomplete detection
  const handleTextareaChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    const newValue = e.target.value;
    setEditingValue(newValue);

    // Use setTimeout to ensure textarea cursor position is updated
    setTimeout(() => {
      const textarea = textareaRef.current;
      if (!textarea) {
        return;
      }

      const cursorPos = textarea.selectionStart;
      const textBeforeCursor = newValue.substring(0, cursorPos);

      // Find the last occurrence of "{{" before cursor
      const lastTriggerIndex = textBeforeCursor.lastIndexOf('{{');

      if (lastTriggerIndex !== -1) {
        // Check if we're still inside the variable reference (no closing }})
        const textAfterTrigger = textBeforeCursor.substring(lastTriggerIndex);
        const hasClosing = textAfterTrigger.includes('}}');

        if (!hasClosing) {
          // Extract the query after "{{"
          const query = textAfterTrigger.substring(2);

          setAutocompleteQuery(query);
          setTriggerIndex(lastTriggerIndex);
          setShowAutocomplete(true);
          return;
        }
      }

      // If we get here, close autocomplete
      setShowAutocomplete(false);
      setAutocompleteQuery('');
      setTriggerIndex(-1);
    }, 0);
  };

  // Handle autocomplete selection
  const handleAutocompleteSelect = (suggestion: VariableSuggestion) => {
    if (triggerIndex === -1) {
      return;
    }

    const textarea = textareaRef.current;
    if (!textarea) {
      return;
    }

    const currentValue = editingValue || '';
    const cursorPos = textarea.selectionStart;

    // Replace from "{{" to cursor position with the selected variable
    const beforeTrigger = currentValue.substring(0, triggerIndex);
    const afterCursor = currentValue.substring(cursorPos);
    const newValue = `${beforeTrigger}{{${suggestion.value}}}${afterCursor}`;

    setEditingValue(newValue);

    // Close autocomplete
    setShowAutocomplete(false);
    setAutocompleteQuery('');
    setTriggerIndex(-1);

    // Set cursor position after the inserted variable
    setTimeout(() => {
      if (textarea) {
        const newCursorPos = beforeTrigger.length + suggestion.value.length + 4; // 4 for {{ and }}
        textarea.selectionStart = newCursorPos;
        textarea.selectionEnd = newCursorPos;
        textarea.focus();
      }
    }, 0);
  };

  // Generate and filter suggestions
  const allSuggestions = composeVariableSuggestions(previousSteps);
  const filteredSuggestions = filterSuggestions(
    allSuggestions,
    autocompleteQuery
  );
  const groupedSuggestions = groupSuggestions(filteredSuggestions);

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

      {/* Value to Switch On */}
      <div className="mb-4">
        <div className="text-sm font-medium text-muted-foreground mb-2">
          Value to Switch On
        </div>
        <MappingValueInput
          value={switchValue}
          onChange={(val) => updateSwitchValue(String(val))}
          valueType={switchValueType}
          onValueTypeChange={(vt) => {
            // Read the latest form state so sequential calls from
            // MappingValueInput don't overwrite each other.
            const currentArray = getValues(name) || fieldArray;
            const newArray = [...currentArray];
            const valueIndex = newArray.findIndex(
              (item: any) => item.type === 'value'
            );
            if (valueIndex >= 0) {
              newArray[valueIndex] = {
                ...newArray[valueIndex],
                valueType: vt,
              };
            }
            setValue(name, newArray);
          }}
          fieldType="string"
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

      {/* Default Output */}
      <div>
        <div className="text-sm font-medium text-muted-foreground mb-2">
          Default Output
        </div>
        <div className="flex items-center gap-1 border rounded-lg p-2">
          <Input
            data-testid="switch-default-output"
            value={
              typeof defaultOutput === 'object'
                ? JSON.stringify(defaultOutput)
                : defaultOutput
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
                  : defaultOutput
              )
            }
          >
            <Icons.edit className="h-3 w-3" />
          </Button>
        </div>
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
              {editingField?.type === 'value'
                ? 'Switch Value'
                : editingField?.type === 'defaultOutput'
                  ? 'Default Output'
                  : editingField?.type === 'caseMatch'
                    ? 'Match Pattern'
                    : 'Case Output'}
            </DialogTitle>
            <DialogDescription>
              {editingField?.type === 'value'
                ? 'Enter the expression to switch on. Use {{ for variable autocomplete.'
                : editingField?.type === 'caseMatch'
                  ? (() => {
                      const caseItem =
                        editingField.index !== undefined
                          ? casesArray[editingField.index]
                          : null;
                      const matchType = caseItem?.matchType || 'exact';
                      return getMatchDescription(matchType);
                    })()
                  : 'Enter the output value or JSON object. Use {{ for variable autocomplete.'}
            </DialogDescription>
          </DialogHeader>
          <div className="py-4 relative">
            <Textarea
              ref={textareaRef}
              value={editingValue}
              onChange={handleTextareaChange}
              className="font-mono text-sm min-h-[200px]"
              placeholder={
                editingField?.type === 'caseMatch'
                  ? 'Enter match pattern (e.g., US or DE, FR, IT)'
                  : 'Enter value (type {{ for autocomplete)'
              }
            />

            {/* Autocomplete Popover - positioned absolutely */}
            {showAutocomplete && editingField?.type !== 'caseMatch' && (
              <div className="absolute left-0 top-full mt-1 w-80 z-50 rounded-sm border bg-popover text-popover-foreground shadow-md animate-in fade-in-0 zoom-in-95 max-h-[300px] overflow-y-auto">
                {filteredSuggestions.length === 0 ? (
                  <div className="py-6 text-center text-sm">
                    No variables found.
                  </div>
                ) : (
                  <>
                    {groupedSuggestions['Workflow Inputs'].length > 0 && (
                      <div className="p-1">
                        <div className="px-2 py-1.5 text-xs font-medium text-muted-foreground">
                          Workflow Inputs
                        </div>
                        {groupedSuggestions['Workflow Inputs'].map(
                          (suggestion) => (
                            <div
                              key={suggestion.value}
                              onClick={() =>
                                handleAutocompleteSelect(suggestion)
                              }
                              onMouseDown={(e) => e.preventDefault()}
                              className="relative flex cursor-pointer select-none items-center rounded-sm px-2 py-1.5 text-sm outline-none hover:bg-accent hover:text-accent-foreground"
                            >
                              <div className="flex flex-col">
                                <span className="font-mono text-sm">
                                  {suggestion.label}
                                </span>
                                {suggestion.description && (
                                  <span className="text-xs text-muted-foreground">
                                    {suggestion.description}
                                  </span>
                                )}
                              </div>
                            </div>
                          )
                        )}
                      </div>
                    )}

                    {groupedSuggestions['Step Outputs'].length > 0 && (
                      <div className="p-1">
                        <div className="px-2 py-1.5 text-xs font-medium text-muted-foreground">
                          Step Outputs
                        </div>
                        {groupedSuggestions['Step Outputs'].map(
                          (suggestion) => (
                            <div
                              key={suggestion.value}
                              onClick={() =>
                                handleAutocompleteSelect(suggestion)
                              }
                              onMouseDown={(e) => e.preventDefault()}
                              className="relative flex cursor-pointer select-none items-center rounded-sm px-2 py-1.5 text-sm outline-none hover:bg-accent hover:text-accent-foreground"
                            >
                              <div className="flex flex-col">
                                <span className="font-mono text-sm">
                                  {suggestion.label}
                                </span>
                                {suggestion.description && (
                                  <span className="text-xs text-muted-foreground">
                                    {suggestion.description}
                                    {suggestion.type && ` • ${suggestion.type}`}
                                  </span>
                                )}
                              </div>
                            </div>
                          )
                        )}
                      </div>
                    )}
                  </>
                )}
              </div>
            )}
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
