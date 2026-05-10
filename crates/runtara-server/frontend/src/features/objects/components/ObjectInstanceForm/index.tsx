import { useEffect, useState } from 'react';
import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import { Switch } from '@/shared/components/ui/switch';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { toast } from '@/shared/hooks/useToast';
import {
  Instance,
  CreateInstanceRequest,
  UpdateInstanceRequest,
  Schema,
} from '@/generated/RuntaraRuntimeApi';
import {
  useCreateObjectInstanceDto,
  useUpdateObjectInstanceDto,
} from '@/features/objects/hooks/useObjectRecords.ts';
import {
  ColumnDataType,
  getWritableObjectColumns,
  mapColumnTypeToDataType,
} from '@/features/objects/utils/columns';

interface ObjectInstanceDtoFormProps {
  objectSchemaDto: Schema;
  record?: Instance;
  onSuccess: () => void;
}

export function ObjectInstanceDtoForm({
  objectSchemaDto,
  record,
  onSuccess,
}: ObjectInstanceDtoFormProps) {
  const [formValues, setFormValues] = useState<Record<string, any>>({});
  const createRecord = useCreateObjectInstanceDto();
  const updateRecord = useUpdateObjectInstanceDto();

  const isEditing = !!record;

  // Initialize form values from record or default values
  useEffect(() => {
    const initialValues: Record<string, any> = {};

    if (!objectSchemaDto.columns) return;

    getWritableObjectColumns(objectSchemaDto.columns).forEach((column) => {
      const dataType = mapColumnTypeToDataType(column.type);

      if (isEditing && record && record.properties) {
        // Get the value for this property from the record
        const value = record.properties[column.name];

        if (value !== undefined) {
          // Extract the actual value from the object structure if it exists
          if (value !== null && typeof value === 'object' && 'value' in value) {
            initialValues[column.name] = (value as { value: any }).value;
          } else {
            initialValues[column.name] = value;
          }
        } else {
          // Set default values if no value found
          setDefaultValue(initialValues, column.name, dataType);
        }
      } else {
        // Set default values based on type
        setDefaultValue(initialValues, column.name, dataType);
      }
    });

    function setDefaultValue(
      values: Record<string, any>,
      key: string,
      dataType: ColumnDataType
    ) {
      switch (dataType) {
        case 'integer':
          values[key] = 0;
          break;
        case 'decimal':
          values[key] = 0.0;
          break;
        case 'boolean':
          values[key] = false;
          break;
        case 'json':
          values[key] = {};
          break;
        case 'enum':
        case 'string':
        case 'timestamp':
          values[key] = '';
          break;
      }
    }

    setFormValues(initialValues);
  }, [objectSchemaDto, record, isEditing]);

  const handleChange = (fieldName: string, value: any) => {
    setFormValues((prev) => ({
      ...prev,
      [fieldName]: value,
    }));
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();

    if (!objectSchemaDto.columns || !objectSchemaDto.id) {
      toast({
        title: 'Error',
        description: 'Schema is not properly defined',
        variant: 'destructive',
      });
      return;
    }

    // Create properties object for the API request
    const properties: Record<string, any> = {};

    // Create a map of column names to data types for quick lookup
    const columnTypeMap = new Map<string, ColumnDataType>();
    getWritableObjectColumns(objectSchemaDto.columns).forEach((column) => {
      columnTypeMap.set(column.name, mapColumnTypeToDataType(column.type));
    });

    // Process each form value and add it to the properties object
    Object.entries(formValues).forEach(([key, value]) => {
      // Skip undefined values
      if (value === undefined) return;

      // Get the data type for this column
      const dataType = columnTypeMap.get(key);
      if (!dataType) return;

      // Process the value based on the data type
      switch (dataType) {
        case 'integer':
          properties[key] =
            typeof value === 'number' ? value : parseInt(value) || 0;
          break;
        case 'decimal':
          properties[key] =
            typeof value === 'number' ? value : parseFloat(value) || 0;
          break;
        case 'boolean':
          properties[key] = !!value;
          break;
        case 'timestamp':
          // Convert to ISO 8601 if it's a Date object
          if (value instanceof Date) {
            properties[key] = value.toISOString();
          } else {
            properties[key] = value;
          }
          break;
        case 'json':
          properties[key] = value; // Keep as object
          break;
        case 'enum':
        case 'string':
          properties[key] = String(value);
          break;
        default:
          properties[key] = value;
      }
    });

    try {
      if (isEditing && record?.id) {
        const updateRequest: UpdateInstanceRequest = {
          properties,
        };

        await updateRecord.mutateAsync({
          schemaId: objectSchemaDto.id,
          instanceId: record.id,
          data: updateRequest,
        });

        toast({
          title: 'Success',
          description: 'Instance updated successfully',
        });
      } else {
        const createRequest: CreateInstanceRequest = {
          schemaId: objectSchemaDto.id,
          properties,
        };

        await createRecord.mutateAsync({
          schemaId: objectSchemaDto.id,
          data: createRequest,
        });

        toast({
          title: 'Success',
          description: 'Instance created successfully',
        });
      }

      if (onSuccess) {
        onSuccess();
      }
    } catch (error) {
      toast({
        title: 'Error',
        description: (error as Error)?.message || 'An error occurred',
        variant: 'destructive',
      });
    }
  };

  return (
    <form
      onSubmit={handleSubmit}
      className="space-y-6 rounded-2xl bg-card px-4 py-5 sm:px-6"
    >
      {objectSchemaDto.columns &&
        getWritableObjectColumns(objectSchemaDto.columns).map((column) => {
          const dataType = mapColumnTypeToDataType(column.type);
          const isRequired = column.nullable === false;

          return (
            <div key={column.name} className="space-y-2">
              <Label
                htmlFor={`field-${column.name}`}
                className="text-sm font-medium text-foreground"
              >
                {column.name}
                {isRequired && <span className="ml-1 text-destructive">*</span>}
              </Label>

              {dataType === 'boolean' ? (
                <div className="flex items-center space-x-3 rounded-lg bg-muted/20 px-3 py-2">
                  <Switch
                    id={`field-${column.name}`}
                    checked={!!formValues[column.name]}
                    onCheckedChange={(checked) =>
                      handleChange(column.name, checked)
                    }
                  />
                  <span className="text-sm text-muted-foreground">
                    {formValues[column.name] ? 'True' : 'False'}
                  </span>
                </div>
              ) : dataType === 'enum' ? (
                <Select
                  value={formValues[column.name] || ''}
                  onValueChange={(value) => handleChange(column.name, value)}
                >
                  <SelectTrigger
                    id={`field-${column.name}`}
                    className="h-11 rounded-2xl"
                  >
                    <SelectValue placeholder="-- Select --" />
                  </SelectTrigger>
                  <SelectContent>
                    {/* TODO: Extract enum values from schema */}
                    <SelectItem value="">-- Select --</SelectItem>
                  </SelectContent>
                </Select>
              ) : dataType === 'timestamp' ? (
                <Input
                  id={`field-${column.name}`}
                  type="datetime-local"
                  value={formValues[column.name] || ''}
                  onChange={(e) => handleChange(column.name, e.target.value)}
                  className="h-11 rounded-2xl"
                />
              ) : dataType === 'integer' || dataType === 'decimal' ? (
                <Input
                  id={`field-${column.name}`}
                  type="number"
                  step={dataType === 'decimal' ? '0.01' : '1'}
                  value={formValues[column.name] || ''}
                  onChange={(e) => {
                    const value =
                      dataType === 'integer'
                        ? parseInt(e.target.value)
                        : parseFloat(e.target.value);
                    handleChange(column.name, value);
                  }}
                  placeholder={`Enter ${column.name}`}
                  className="h-11 rounded-2xl"
                />
              ) : dataType === 'json' ? (
                <textarea
                  id={`field-${column.name}`}
                  value={JSON.stringify(formValues[column.name] || {}, null, 2)}
                  onChange={(e) => {
                    try {
                      const parsed = JSON.parse(e.target.value);
                      handleChange(column.name, parsed);
                    } catch {
                      // Invalid JSON, keep as string for now
                    }
                  }}
                  placeholder='{"key": "value"}'
                  className="min-h-[120px] w-full rounded-2xl border border-border/50 bg-background px-3 py-2 text-sm font-mono focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                  rows={5}
                />
              ) : (
                <Input
                  id={`field-${column.name}`}
                  type="text"
                  value={formValues[column.name] || ''}
                  onChange={(e) => handleChange(column.name, e.target.value)}
                  placeholder={`Enter ${column.name}`}
                  className="h-11 rounded-2xl"
                />
              )}

              {column.type.startsWith('DECIMAL') && (
                <p className="text-xs text-muted-foreground">
                  Decimal number with up to{' '}
                  {column.type.match(/\d+/)?.[0] || 10} total digits
                </p>
              )}

              {dataType === 'timestamp' && (
                <p className="text-xs text-muted-foreground">UTC timezone</p>
              )}
            </div>
          );
        })}

      <div className="flex flex-col gap-3 border-t border-border/40 pt-6 sm:flex-row sm:items-center sm:justify-between">
        <p className="text-xs text-muted-foreground">
          * Required fields must be filled
        </p>
        <div className="flex flex-col gap-3 sm:flex-row">
          <Button
            type="button"
            variant="ghost"
            onClick={onSuccess}
            className="rounded-full"
          >
            Cancel
          </Button>
          <Button
            type="submit"
            className="h-11 rounded-full px-6"
            disabled={createRecord.isPending || updateRecord.isPending}
          >
            {isEditing ? 'Update Instance' : 'Create Instance'}
          </Button>
        </div>
      </div>
    </form>
  );
}
