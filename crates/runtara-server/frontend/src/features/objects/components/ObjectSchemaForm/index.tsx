import React, { useState } from 'react';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import { FormSection } from '@/shared/components/form-section';
import { ObjectSchemaFormLayout } from './ObjectSchemaFormLayout';
import { FileText, Columns } from 'lucide-react';
import {
  useCreateObjectSchemaDto,
  useUpdateObjectSchemaDto,
} from '../../hooks/useObjectSchemas';
import { toast } from 'sonner';
import {
  Schema,
  CreateSchemaRequest,
  UpdateSchemaRequest,
  ColumnDefinition,
} from '@/generated/RuntaraRuntimeApi';
import { SQLPreview } from '../SQLPreview';
import {
  ObjectSchemaFieldsTable,
  FieldDefinition,
} from './ObjectSchemaFieldsTable';

const generateFieldId = () =>
  `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 7)}`;

const createEmptyField = (): FieldDefinition => ({
  __id: generateFieldId(),
  name: '',
  dataType: 'string',
  nullable: true,
  unique: false,
  default: undefined,
});

// Generate table name from schema name
const generateTableName = (schemaName: string): string => {
  if (!schemaName) return '';
  return (
    schemaName
      .toLowerCase()
      .replace(/\s+/g, '_')
      .replace(/[^a-z0-9_]/g, '') + 's'
  );
};

// Convert columns from Schema to FieldDefinition
const mapColumnsToFields = (
  columns?: ColumnDefinition[]
): FieldDefinition[] => {
  if (!columns || columns.length === 0) return [];

  return columns.map((col) => {
    const base: FieldDefinition = {
      __id: generateFieldId(),
      name: col.name,
      dataType: col.type,
      nullable: col.nullable !== false,
      unique: col.unique || false,
      default: col.default || undefined,
    };

    // Extract type-specific fields
    if (col.type === 'decimal') {
      base.precision = col.precision;
      base.scale = col.scale;
    } else if (col.type === 'enum') {
      base.values = col.values;
    }

    return base;
  });
};

// Map UI field type to PostgreSQL type
const mapFieldTypeToPostgresType = (field: FieldDefinition): string => {
  switch (field.dataType) {
    case 'string':
      return 'TEXT';
    case 'integer':
      return 'INTEGER';
    case 'decimal':
      return `DECIMAL(${field.precision || 19},${field.scale || 4})`;
    case 'boolean':
      return 'BOOLEAN';
    case 'timestamp':
      return 'TIMESTAMP';
    case 'json':
      return 'JSONB';
    case 'enum':
      return 'TEXT'; // With CHECK constraint (handled separately)
    default:
      return 'TEXT';
  }
};

// Convert form fields to ColumnDefinition array
const convertFieldsToColumns = (
  fields: FieldDefinition[]
): ColumnDefinition[] => {
  return fields
    .filter((field) => field.name.trim())
    .map((field) => {
      const base = {
        name: field.name,
        nullable: field.nullable,
        unique: field.unique,
        default: field.default || null,
      };

      // Build the correct union type based on dataType
      switch (field.dataType) {
        case 'string':
          return { ...base, type: 'string' as const };
        case 'integer':
          return { ...base, type: 'integer' as const };
        case 'decimal':
          return {
            ...base,
            type: 'decimal' as const,
            precision: field.precision,
            scale: field.scale,
          };
        case 'boolean':
          return { ...base, type: 'boolean' as const };
        case 'timestamp':
          return { ...base, type: 'timestamp' as const };
        case 'json':
          return { ...base, type: 'json' as const };
        case 'enum':
          return {
            ...base,
            type: 'enum' as const,
            values: field.values || [],
          };
        default:
          return { ...base, type: 'string' as const };
      }
    });
};

interface ObjectSchemaDtoFormProps {
  objectSchemaDto?: Schema;
  onSuccess: () => void;
  onDelete?: () => void;
  isDeleting?: boolean;
}

interface FormErrors {
  name?: string;
  tableName?: string;
  fields?: string;
}

export function ObjectSchemaDtoForm({
  objectSchemaDto,
  onSuccess,
  onDelete,
  isDeleting,
}: ObjectSchemaDtoFormProps) {
  const [name, setName] = useState(objectSchemaDto?.name || '');
  const [description, setDescription] = useState(
    objectSchemaDto?.description || ''
  );
  const [tableName, setTableName] = useState(objectSchemaDto?.tableName || '');
  const [fields, setFields] = useState<FieldDefinition[]>(
    mapColumnsToFields(objectSchemaDto?.columns)
  );
  const [errors, setErrors] = useState<FormErrors>({});
  const [touched, setTouched] = useState<Record<string, boolean>>({});

  const createObjectSchemaDto = useCreateObjectSchemaDto();
  const updateObjectSchemaDto = useUpdateObjectSchemaDto();

  const isEditing = !!objectSchemaDto;

  // Auto-generate table name from schema name (only for new schemas)
  React.useEffect(() => {
    if (!isEditing && name) {
      setTableName(generateTableName(name));
    }
  }, [name, isEditing]);

  const addField = () => {
    setFields([...fields, createEmptyField()]);
  };

  const validateName = (value: string): string | undefined => {
    if (!value.trim()) {
      return 'Schema name is required';
    }
    if (!/^[a-zA-Z][a-zA-Z0-9_]*$/.test(value)) {
      return 'Must start with a letter and contain only letters, numbers, and underscores';
    }
    return undefined;
  };

  const validateTableName = (value: string): string | undefined => {
    if (!isEditing && !value.trim()) {
      return 'Table name is required';
    }
    return undefined;
  };

  const validateFields = (): string | undefined => {
    if (fields.length === 0) {
      return 'At least one column is required';
    }

    for (let i = 0; i < fields.length; i++) {
      const field = fields[i];

      if (!field.name?.trim()) {
        return `Column ${i + 1}: Name is required`;
      }

      if (!/^[a-zA-Z][a-zA-Z0-9_]*$/.test(field.name)) {
        return `Column ${i + 1}: Name must start with a letter and contain only letters, numbers, and underscores`;
      }

      if (
        field.dataType === 'enum' &&
        (!field.values || field.values.length === 0)
      ) {
        return `Column ${i + 1}: Enum type must have at least one value`;
      }

      if (field.dataType === 'decimal') {
        if (
          field.precision &&
          (field.precision < 1 || field.precision > 1000)
        ) {
          return `Column ${i + 1}: Precision must be between 1 and 1000`;
        }
        if (field.scale && field.precision && field.scale > field.precision) {
          return `Column ${i + 1}: Scale cannot be greater than precision`;
        }
      }
    }

    // Check for duplicate column names
    const columnNames = fields.map((f) => f.name.toLowerCase());
    const duplicates = columnNames.filter(
      (colName, index) => columnNames.indexOf(colName) !== index
    );
    if (duplicates.length > 0) {
      return `Duplicate column names found: ${duplicates.join(', ')}`;
    }

    return undefined;
  };

  const validateForm = (): FormErrors => {
    const newErrors: FormErrors = {
      name: validateName(name),
      tableName: validateTableName(tableName),
      fields: validateFields(),
    };
    return newErrors;
  };

  const hasErrors = (formErrors: FormErrors): boolean => {
    return Object.values(formErrors).some((error) => error !== undefined);
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();

    // Mark all fields as touched on submit
    setTouched({ name: true, tableName: true, fields: true });

    const formErrors = validateForm();
    setErrors(formErrors);

    if (hasErrors(formErrors)) {
      return;
    }

    const columns = convertFieldsToColumns(fields);

    try {
      if (isEditing && objectSchemaDto?.id) {
        const updateRequest: UpdateSchemaRequest = {
          name,
          description: description || '',
          columns,
        };

        await updateObjectSchemaDto.mutateAsync({
          id: objectSchemaDto.id,
          data: updateRequest,
        });

        toast.success('Schema updated successfully');
      } else {
        const createRequest: CreateSchemaRequest = {
          name,
          tableName,
          description: description || '',
          columns,
          indexes: [], // TODO: Add index support
        };

        await createObjectSchemaDto.mutateAsync(createRequest);

        toast.success('Schema created successfully');
      }

      if (onSuccess) {
        onSuccess();
      }
    } catch {
      // Error toast is handled by useCustomMutation's onError
    }
  };

  const isPending =
    createObjectSchemaDto.isPending || updateObjectSchemaDto.isPending;

  const fieldCount = fields.filter((f) => f.name.trim()).length;
  const metadata = isEditing
    ? [
        fieldCount
          ? `${fieldCount} ${fieldCount === 1 ? 'column' : 'columns'}`
          : null,
        objectSchemaDto?.id ? `ID: ${objectSchemaDto.id}` : null,
      ]
    : undefined;

  return (
    <form onSubmit={handleSubmit}>
      <ObjectSchemaFormLayout
        title={isEditing ? 'Edit object type' : 'Create object type'}
        schemaName={isEditing ? objectSchemaDto?.name : undefined}
        isLoading={isPending}
        submitLabel={isEditing ? 'Save changes' : 'Create schema'}
        loadingLabel={isEditing ? 'Saving...' : 'Creating...'}
        onDelete={onDelete}
        isDeleting={isDeleting}
        metadata={metadata}
      >
        <div className="space-y-5">
          {/* Basic Information */}
          <FormSection
            title="Basic Information"
            description="Define the schema name and database table configuration"
            icon={FileText}
          >
            {!isEditing && (
              <>
                <div className="space-y-2">
                  <Label
                    htmlFor="name"
                    className="text-sm font-medium text-foreground"
                  >
                    Schema Name
                  </Label>
                  <Input
                    id="name"
                    value={name}
                    onChange={(e) => {
                      setName(e.target.value);
                      if (touched.name) {
                        setErrors((prev) => ({
                          ...prev,
                          name: validateName(e.target.value),
                        }));
                      }
                    }}
                    onBlur={() => {
                      setTouched((prev) => ({ ...prev, name: true }));
                      setErrors((prev) => ({
                        ...prev,
                        name: validateName(name),
                      }));
                    }}
                    placeholder="e.g., Product, Customer, Order"
                    className={
                      touched.name && errors.name
                        ? 'border-red-500 focus-visible:ring-red-500'
                        : ''
                    }
                  />
                  {touched.name && errors.name ? (
                    <p className="text-xs text-red-500">{errors.name}</p>
                  ) : (
                    <p className="text-xs text-muted-foreground">
                      Letters, numbers, underscores only. Must start with
                      letter.
                    </p>
                  )}
                </div>

                <div className="space-y-2">
                  <Label
                    htmlFor="tableName"
                    className="text-sm font-medium text-foreground"
                  >
                    Table Name
                  </Label>
                  <Input
                    id="tableName"
                    value={tableName}
                    onChange={(e) => {
                      setTableName(e.target.value);
                      if (touched.tableName) {
                        setErrors((prev) => ({
                          ...prev,
                          tableName: validateTableName(e.target.value),
                        }));
                      }
                    }}
                    onBlur={() => {
                      setTouched((prev) => ({ ...prev, tableName: true }));
                      setErrors((prev) => ({
                        ...prev,
                        tableName: validateTableName(tableName),
                      }));
                    }}
                    placeholder="e.g., products, customers, orders"
                    className={
                      touched.tableName && errors.tableName
                        ? 'border-red-500 focus-visible:ring-red-500'
                        : ''
                    }
                  />
                  {touched.tableName && errors.tableName ? (
                    <p className="text-xs text-red-500">{errors.tableName}</p>
                  ) : (
                    <p className="text-xs text-muted-foreground">
                      Internal database table name (will be prefixed with tenant
                      ID)
                    </p>
                  )}
                </div>
              </>
            )}

            <div className="space-y-2">
              <Label
                htmlFor="description"
                className="text-sm font-medium text-foreground"
              >
                Description
              </Label>
              <Input
                id="description"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                placeholder="Brief description of this schema"
              />
            </div>
          </FormSection>

          {/* Columns */}
          <FormSection
            title="Columns"
            description="Define the structure and data types for this schema"
            icon={Columns}
          >
            <ObjectSchemaFieldsTable
              fields={fields}
              onFieldsChange={(newFields) => {
                setFields(newFields);
                if (touched.fields) {
                  setErrors((prev) => ({
                    ...prev,
                    fields: validateFields(),
                  }));
                }
              }}
              onAddField={addField}
            />
            {touched.fields && errors.fields && (
              <p className="text-xs text-red-500 mt-2">{errors.fields}</p>
            )}
          </FormSection>

          {/* SQL Preview */}
          {fields.length > 0 && (
            <SQLPreview
              schemaName={name}
              tableName={tableName}
              columns={fields.map((f) => ({
                name: f.name,
                type: mapFieldTypeToPostgresType(f),
                nullable: f.nullable,
                unique: f.unique,
                default: f.default,
              }))}
              indexes={[]}
            />
          )}
        </div>
      </ObjectSchemaFormLayout>
    </form>
  );
}
