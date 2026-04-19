import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import { Plus, Trash2 } from 'lucide-react';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { VariableType } from '@/generated/RuntaraRuntimeApi';

// UI-specific variable type with name field for editing
export interface UIVariable {
  name: string;
  value: any;
  type: VariableType;
  description?: string | null;
}

// Re-export for convenience
export { VariableType };

/**
 * Converts a string value to the appropriate JSON type based on the variable type.
 * This ensures that booleans are stored as true/false, numbers as numbers, etc.
 */
function convertValueToType(value: any, type: VariableType): any {
  // If value is already the correct type, return as-is
  if (value === null || value === undefined) {
    return value;
  }

  const stringValue = typeof value === 'string' ? value : String(value);

  switch (type) {
    case VariableType.Boolean:
      if (typeof value === 'boolean') return value;
      return stringValue.toLowerCase() === 'true';

    case VariableType.Number: {
      if (typeof value === 'number') return value;
      const num = parseFloat(stringValue);
      return isNaN(num) ? 0 : num;
    }

    case VariableType.Integer: {
      if (typeof value === 'number' && Number.isInteger(value)) return value;
      const int = parseInt(stringValue, 10);
      return isNaN(int) ? 0 : int;
    }

    case VariableType.Array:
      if (Array.isArray(value)) return value;
      try {
        const parsed = JSON.parse(stringValue);
        return Array.isArray(parsed) ? parsed : [];
      } catch {
        return [];
      }

    case VariableType.Object:
      if (typeof value === 'object' && !Array.isArray(value)) return value;
      try {
        const parsed = JSON.parse(stringValue);
        return typeof parsed === 'object' && !Array.isArray(parsed)
          ? parsed
          : {};
      } catch {
        return {};
      }

    case VariableType.String:
    case VariableType.File:
    default:
      return stringValue;
  }
}

/**
 * Formats a typed value for display in an input field.
 */
function formatValueForDisplay(value: any): string {
  if (value === null || value === undefined) return '';
  if (typeof value === 'string') return value;
  return JSON.stringify(value);
}

interface VariablesEditorProps {
  variables: UIVariable[];
  onChange: (variables: UIVariable[]) => void;
  readOnly?: boolean;
  hideLabel?: boolean;
}

export function VariablesEditor({
  variables,
  onChange,
  readOnly = false,
  hideLabel = false,
}: VariablesEditorProps) {
  const handleAdd = () => {
    onChange([
      ...variables,
      { name: '', value: '', type: VariableType.String } as UIVariable,
    ]);
  };

  const handleRemove = (index: number) => {
    const newVariables = [...variables];
    newVariables.splice(index, 1);
    onChange(newVariables);
  };

  const handleChange = (index: number, field: keyof UIVariable, value: any) => {
    const newVariables = [...variables];
    const currentVariable = newVariables[index];

    if (field === 'value') {
      // Convert the value to the appropriate type based on the variable's type
      const convertedValue = convertValueToType(value, currentVariable.type);
      newVariables[index] = { ...currentVariable, value: convertedValue };
    } else if (field === 'type') {
      // When type changes, convert the existing value to the new type
      const newType = value as VariableType;
      const convertedValue = convertValueToType(currentVariable.value, newType);
      newVariables[index] = {
        ...currentVariable,
        type: newType,
        value: convertedValue,
      };
    } else {
      newVariables[index] = { ...currentVariable, [field]: value };
    }

    onChange(newVariables);
  };

  return (
    <div className="space-y-2">
      {!hideLabel && (
        <Label className="text-sm font-medium">Variables (Constants)</Label>
      )}
      <div className="border rounded-lg">
        <table className="w-full">
          <thead>
            <tr className="border-b">
              <th className="text-left p-2 text-sm font-medium text-muted-foreground">
                Name
              </th>
              <th className="text-left p-2 text-sm font-medium text-muted-foreground">
                Value
              </th>
              <th className="text-left p-2 text-sm font-medium text-muted-foreground">
                Type
              </th>
              <th className="text-left p-2 text-sm font-medium text-muted-foreground">
                Description
              </th>
              {!readOnly && (
                <th className="w-16 text-center p-2 text-sm font-medium text-muted-foreground">
                  Actions
                </th>
              )}
            </tr>
          </thead>
          <tbody>
            {variables.map((variable, index) => (
              <tr key={index} className="border-b hover:bg-muted/30">
                <td className="p-2">
                  <Input
                    value={variable.name}
                    onChange={(e) =>
                      handleChange(index, 'name', e.target.value)
                    }
                    placeholder="myVar"
                    disabled={readOnly}
                    className="font-mono text-sm border-0 p-1 h-auto focus-visible:ring-0 focus-visible:ring-offset-0"
                  />
                </td>
                <td className="p-2">
                  {variable.type === VariableType.Boolean ? (
                    <Select
                      value={String(variable.value === true)}
                      onValueChange={(val) =>
                        handleChange(index, 'value', val === 'true')
                      }
                      disabled={readOnly}
                    >
                      <SelectTrigger className="h-7 font-mono text-sm border-0 focus:ring-0 focus:ring-offset-0">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="true">true</SelectItem>
                        <SelectItem value="false">false</SelectItem>
                      </SelectContent>
                    </Select>
                  ) : (
                    <Input
                      value={formatValueForDisplay(variable.value)}
                      onChange={(e) =>
                        handleChange(index, 'value', e.target.value)
                      }
                      placeholder="constant value"
                      disabled={readOnly}
                      className="font-mono text-sm border-0 p-1 h-auto focus-visible:ring-0 focus-visible:ring-offset-0"
                    />
                  )}
                </td>
                <td className="p-2">
                  <Select
                    value={variable.type || VariableType.String}
                    onValueChange={(value) =>
                      handleChange(index, 'type', value as VariableType)
                    }
                    disabled={readOnly}
                  >
                    <SelectTrigger className="h-7 border-0 focus:ring-0 focus:ring-offset-0">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value={VariableType.String}>
                        String
                      </SelectItem>
                      <SelectItem value={VariableType.Number}>
                        Number
                      </SelectItem>
                      <SelectItem value={VariableType.Integer}>
                        Integer
                      </SelectItem>
                      <SelectItem value={VariableType.Boolean}>
                        Boolean
                      </SelectItem>
                      <SelectItem value={VariableType.Object}>
                        Object
                      </SelectItem>
                      <SelectItem value={VariableType.Array}>Array</SelectItem>
                      <SelectItem value={VariableType.File}>File</SelectItem>
                    </SelectContent>
                  </Select>
                </td>
                <td className="p-2">
                  <Input
                    value={variable.description || ''}
                    onChange={(e) =>
                      handleChange(index, 'description', e.target.value || null)
                    }
                    placeholder="Optional description"
                    disabled={readOnly}
                    className="text-sm border-0 p-1 h-auto focus-visible:ring-0 focus-visible:ring-offset-0"
                  />
                </td>
                {!readOnly && (
                  <td className="p-2 text-center">
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      onClick={() => handleRemove(index)}
                      className="h-6 w-6 p-0"
                    >
                      <Trash2 className="h-3 w-3" />
                    </Button>
                  </td>
                )}
              </tr>
            ))}
            {variables.length === 0 && (
              <tr>
                <td
                  colSpan={readOnly ? 4 : 5}
                  className="p-4 text-center text-sm text-muted-foreground"
                >
                  No variables defined. Variables are constants shared in your
                  workflow.
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
      {!readOnly && (
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={handleAdd}
          className="w-full"
        >
          <Plus className="h-4 w-4 mr-2" />
          Add Variable
        </Button>
      )}
    </div>
  );
}
