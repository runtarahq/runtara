import { useEffect, useId, useMemo, useState, type ReactNode } from 'react';
import { Button } from '@/shared/components/ui/button';
import { Checkbox } from '@/shared/components/ui/checkbox';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import { Plus, Settings2, Trash2 } from 'lucide-react';
import { cn } from '@/lib/utils';
import { validateSchemaFieldsWithRust } from '@/features/workflows/utils/rust-workflow-validation';
import type { RustSchemaFieldsValidationError } from '@/features/workflows/utils/rust-workflow-validation';
import {
  KNOWN_SCHEMA_FIELD_KEYS,
  buildSchemaFromFields as buildSchemaObjectFromFields,
  parseSchema as parseRawSchema,
} from '@/features/workflows/utils/schema';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { Textarea } from '@/shared/components/ui/textarea';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from '@/shared/components/ui/dialog';

const SUPPORTED_TYPES: { label: string; value: string }[] = [
  { label: 'String', value: 'string' },
  { label: 'Number', value: 'number' },
  { label: 'Integer', value: 'integer' },
  { label: 'Boolean', value: 'boolean' },
  { label: 'Object', value: 'object' },
  { label: 'Array', value: 'array' },
  { label: 'File', value: 'file' },
];

export type SchemaField = {
  name: string;
  type: string;
  required: boolean;
  description: string;
  defaultValue?: any;
  enum?: any[];
  example?: any;
  items?: any;
  nullable?: boolean;
  label?: string;
  placeholder?: string;
  order?: number;
  format?: string;
  min?: number;
  max?: number;
  pattern?: string;
  properties?: SchemaField[];
  visibleWhen?: {
    field: string;
    equals?: any;
    notEquals?: any;
  };
  extensions?: Record<string, any>;
};

const ADVANCED_SCHEMA_KEYS = new Set([
  'example',
  'items',
  'label',
  'placeholder',
  'order',
  'format',
  'min',
  'max',
  'pattern',
  'properties',
  'visibleWhen',
]);

function formatFieldValue(value: any): string {
  if (value === undefined || value === null) return '';
  if (typeof value === 'string') return value;
  return JSON.stringify(value);
}

function parseDefaultValue(raw: string, type: string | undefined): any {
  if (raw.trim() === '') return undefined;

  if (type === 'number') {
    const value = Number(raw);
    return Number.isFinite(value) ? value : raw;
  }

  if (type === 'integer') {
    const value = parseInt(raw, 10);
    return Number.isFinite(value) ? value : raw;
  }

  if (type === 'boolean') {
    if (raw.toLowerCase() === 'true') return true;
    if (raw.toLowerCase() === 'false') return false;
    return raw;
  }

  if (type === 'object' || type === 'array') {
    try {
      return JSON.parse(raw);
    } catch {
      return raw;
    }
  }

  return raw;
}

function formatEnumValue(values: any[] | undefined): string {
  if (!Array.isArray(values) || values.length === 0) return '';
  const hasStructuredValue = values.some(
    (value) => value !== null && typeof value === 'object'
  );
  if (hasStructuredValue) {
    return JSON.stringify(values);
  }
  return values.map((value) => String(value)).join(', ');
}

function parseEnumValue(raw: string): any[] | undefined {
  const trimmed = raw.trim();
  if (!trimmed) return undefined;

  if (trimmed.startsWith('[')) {
    try {
      const parsed = JSON.parse(trimmed);
      return Array.isArray(parsed) ? parsed : [parsed];
    } catch {
      // Fall back to comma-separated values below.
    }
  }

  const values = raw
    .split(',')
    .map((value) => value.trim())
    .filter(Boolean);
  return values.length > 0 ? values : undefined;
}

/**
 * Display format hints documented for `string` fields in the DSL
 * (`crates/runtara-dsl/src/schema_types.rs`, `SchemaField::format`).
 * Unknown formats are preserved as custom values.
 */
const KNOWN_STRING_FORMATS = [
  'textarea',
  'date',
  'datetime',
  'email',
  'url',
  'tel',
  'color',
  'password',
  'markdown',
];

const FORMAT_NONE = '__none__';

export type VisibleWhenOperator = 'equals' | 'notEquals';

export type AdvancedSchemaDraft = {
  label: string;
  placeholder: string;
  order: string;
  min: string;
  max: string;
  pattern: string;
  format: string;
  /** Strict JSON text; empty string means unset. */
  example: string;
  /** Element type for array fields (raw `items.type`). */
  itemsType: string;
  /** Parsed `items.properties` when the element type is object. */
  itemsProperties: SchemaField[];
  /** All other keys of the raw `items` object, preserved verbatim. */
  itemsRest: Record<string, any>;
  properties: SchemaField[];
  visibleWhenField: string;
  visibleWhenRows: { operator: VisibleWhenOperator; value: string }[];
  /** Strict JSON object text for unknown extension keys; empty means none. */
  extensionsText: string;
};

export type AdvancedSchemaDraftErrors = Partial<
  Record<'order' | 'min' | 'max' | 'example' | 'visibleWhen' | 'extensions', string>
>;

/**
 * Format a JSON value for a lenient single-line input: plain strings render
 * raw, everything else (and strings that would re-parse as JSON) render as
 * JSON so the round-trip stays exact.
 */
function formatLooseValue(value: any): string {
  if (value === undefined) return '';
  if (typeof value === 'string') {
    const trimmed = value.trim();
    if (!trimmed) return value;
    try {
      JSON.parse(trimmed);
      return JSON.stringify(value);
    } catch {
      return value;
    }
  }
  return JSON.stringify(value);
}

/** Parse a lenient input: valid JSON wins, anything else is a string. */
function parseLooseValue(raw: string): any {
  const trimmed = raw.trim();
  if (!trimmed) return '';
  try {
    return JSON.parse(trimmed);
  } catch {
    return raw;
  }
}

function toEditorFields(parsed: ReturnType<typeof parseRawSchema>): SchemaField[] {
  return parsed.map((property) => ({
    ...property,
    type: property.type || 'string',
    required: !!property.required,
    description: property.description || '',
  })) as SchemaField[];
}

export function createAdvancedSchemaDraft(field: SchemaField): AdvancedSchemaDraft {
  const rawItems =
    field.items && typeof field.items === 'object' && !Array.isArray(field.items)
      ? (field.items as Record<string, any>)
      : undefined;
  const {
    type: itemsType,
    properties: itemsRawProperties,
    ...itemsRest
  } = rawItems ?? {};

  const visibleWhenRows: AdvancedSchemaDraft['visibleWhenRows'] = [];
  if (field.visibleWhen) {
    if (field.visibleWhen.equals !== undefined) {
      visibleWhenRows.push({
        operator: 'equals',
        value: formatLooseValue(field.visibleWhen.equals),
      });
    }
    if (field.visibleWhen.notEquals !== undefined) {
      visibleWhenRows.push({
        operator: 'notEquals',
        value: formatLooseValue(field.visibleWhen.notEquals),
      });
    }
  }

  return {
    label: field.label ?? '',
    placeholder: field.placeholder ?? '',
    order: field.order != null ? String(field.order) : '',
    min: field.min != null ? String(field.min) : '',
    max: field.max != null ? String(field.max) : '',
    pattern: field.pattern ?? '',
    format: field.format ?? '',
    example: field.example !== undefined ? JSON.stringify(field.example) : '',
    itemsType: typeof itemsType === 'string' ? itemsType : 'string',
    itemsProperties: itemsRawProperties
      ? toEditorFields(parseRawSchema(itemsRawProperties))
      : [],
    itemsRest,
    properties: field.properties ?? [],
    visibleWhenField: field.visibleWhen?.field ?? '',
    visibleWhenRows,
    extensionsText:
      field.extensions && Object.keys(field.extensions).length > 0
        ? JSON.stringify(field.extensions, null, 2)
        : '',
  };
}

/**
 * Apply a structured advanced-schema draft back onto a field. Returns the
 * updated field, or `null` plus per-section errors when the draft is invalid.
 * The output keeps exactly the shapes `parseSchema`/`buildSchemaFromFields`
 * round-trip: known keys live on the field, unknown keys in `extensions`.
 */
export function applyAdvancedSchemaDraft(
  field: SchemaField,
  draft: AdvancedSchemaDraft
): { field: SchemaField | null; errors: AdvancedSchemaDraftErrors } {
  const errors: AdvancedSchemaDraftErrors = {};
  const next: SchemaField = { ...field };
  for (const key of ADVANCED_SCHEMA_KEYS) {
    delete (next as any)[key];
  }
  delete next.extensions;

  if (draft.label) next.label = draft.label;
  if (draft.placeholder) next.placeholder = draft.placeholder;
  if (draft.format) next.format = draft.format;
  if (draft.pattern) next.pattern = draft.pattern;

  const orderRaw = draft.order.trim();
  if (orderRaw) {
    const order = Number(orderRaw);
    if (!Number.isInteger(order)) {
      errors.order = 'Order must be an integer.';
    } else {
      next.order = order;
    }
  }

  const minRaw = draft.min.trim();
  if (minRaw) {
    const min = Number(minRaw);
    if (!Number.isFinite(min)) {
      errors.min = 'Min must be a number.';
    } else {
      next.min = min;
    }
  }

  const maxRaw = draft.max.trim();
  if (maxRaw) {
    const max = Number(maxRaw);
    if (!Number.isFinite(max)) {
      errors.max = 'Max must be a number.';
    } else {
      next.max = max;
    }
  }

  const exampleRaw = draft.example.trim();
  if (exampleRaw) {
    try {
      next.example = JSON.parse(exampleRaw);
    } catch {
      errors.example = 'Example must be valid JSON (quote strings, e.g. "text").';
    }
  }

  if ((field.type || 'string') === 'array') {
    const items: Record<string, any> = {
      ...draft.itemsRest,
      type: draft.itemsType || 'string',
    };
    if (items.type === 'object' && draft.itemsProperties.length > 0) {
      items.properties = buildSchemaObjectFromFields(draft.itemsProperties as any);
    }
    next.items = items;
  } else if (field.items !== undefined) {
    // Non-array fields keep a pre-existing items definition untouched.
    next.items = field.items;
  }

  if (draft.properties.length > 0) {
    next.properties = draft.properties;
  }

  if (draft.visibleWhenRows.length > 0) {
    const visibleWhenField = draft.visibleWhenField.trim();
    if (!visibleWhenField) {
      errors.visibleWhen = 'Enter the sibling field name the visibility rule checks.';
    } else {
      const visibleWhen: NonNullable<SchemaField['visibleWhen']> = {
        field: visibleWhenField,
      };
      const seen = new Set<VisibleWhenOperator>();
      for (const row of draft.visibleWhenRows) {
        if (seen.has(row.operator)) {
          errors.visibleWhen = 'Each operator can be used at most once.';
          break;
        }
        seen.add(row.operator);
        if (row.operator === 'equals') {
          visibleWhen.equals = parseLooseValue(row.value);
        } else {
          visibleWhen.notEquals = parseLooseValue(row.value);
        }
      }
      if (!errors.visibleWhen) {
        next.visibleWhen = visibleWhen;
      }
    }
  }

  const extensionsRaw = draft.extensionsText.trim();
  if (extensionsRaw) {
    try {
      const parsed = JSON.parse(extensionsRaw);
      if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
        errors.extensions = 'Extensions must be a JSON object.';
      } else {
        const reserved = Object.keys(parsed).filter((key) =>
          KNOWN_SCHEMA_FIELD_KEYS.has(key)
        );
        if (reserved.length > 0) {
          errors.extensions = `Use the structured controls above for: ${reserved.join(
            ', '
          )}.`;
        } else if (Object.keys(parsed).length > 0) {
          next.extensions = parsed;
        }
      }
    } catch {
      errors.extensions = 'Extensions must be valid JSON.';
    }
  }

  if (Object.keys(errors).length > 0) {
    return { field: null, errors };
  }
  return { field: next, errors };
}

function fieldHasAdvancedMetadata(field: SchemaField): boolean {
  if (field.extensions && Object.keys(field.extensions).length > 0) {
    return true;
  }
  for (const key of ADVANCED_SCHEMA_KEYS) {
    const value = (field as any)[key];
    if (key === 'properties') {
      if (Array.isArray(value) && value.length > 0) return true;
      continue;
    }
    if (value !== undefined && value !== '') return true;
  }
  return false;
}

function DialogSection({
  title,
  hint,
  children,
}: {
  title: string;
  hint?: string;
  children: ReactNode;
}) {
  return (
    <section className="space-y-2">
      <div>
        <h3 className="text-sm font-medium">{title}</h3>
        {hint && <p className="text-xs text-muted-foreground">{hint}</p>}
      </div>
      {children}
    </section>
  );
}

function NestedFieldsEditor({
  label,
  fields,
  onChange,
  readOnly,
  showEnum,
}: {
  label: string;
  fields: SchemaField[];
  onChange: (fields: SchemaField[]) => void;
  readOnly: boolean;
  showEnum: boolean;
}) {
  return (
    <div className="rounded-md border-l-2 border-muted bg-muted/20 p-2 pl-3">
      <SchemaFieldsEditor
        label={label}
        fields={fields}
        onChange={onChange}
        readOnly={readOnly}
        showEnum={showEnum}
        emptyMessage="No nested fields defined."
      />
    </div>
  );
}

function AdvancedSchemaFieldDialog({
  field,
  readOnly,
  onApply,
  siblingNames,
  showEnum,
}: {
  field: SchemaField;
  readOnly: boolean;
  onApply: (field: SchemaField) => void;
  siblingNames: string[];
  showEnum: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState<AdvancedSchemaDraft | null>(null);
  const [errors, setErrors] = useState<AdvancedSchemaDraftErrors>({});
  const baseId = useId();
  const hasAdvanced = useMemo(() => fieldHasAdvancedMetadata(field), [field]);
  const fieldType = field.type || 'string';

  const patternWarning = useMemo(() => {
    if (!draft?.pattern) return null;
    try {
      new RegExp(draft.pattern);
      return null;
    } catch {
      return 'Not a valid regular expression.';
    }
  }, [draft?.pattern]);

  const formatOptions = useMemo(() => {
    const options = [...KNOWN_STRING_FORMATS];
    if (draft?.format && !options.includes(draft.format)) {
      options.push(draft.format);
    }
    return options;
  }, [draft?.format]);

  const extensionCount = field.extensions
    ? Object.keys(field.extensions).length
    : 0;

  const handleOpenChange = (nextOpen: boolean) => {
    if (nextOpen) {
      setDraft(createAdvancedSchemaDraft(field));
      setErrors({});
    }
    setOpen(nextOpen);
  };

  const updateDraft = (patch: Partial<AdvancedSchemaDraft>) => {
    setDraft((current) => (current ? { ...current, ...patch } : current));
    setErrors({});
  };

  const addVisibleWhenRow = () => {
    if (!draft) return;
    const used = new Set(draft.visibleWhenRows.map((row) => row.operator));
    const operator: VisibleWhenOperator = used.has('equals')
      ? 'notEquals'
      : 'equals';
    updateDraft({
      visibleWhenRows: [...draft.visibleWhenRows, { operator, value: '' }],
    });
  };

  const handleApply = () => {
    if (!draft) return;
    const result = applyAdvancedSchemaDraft(field, draft);
    if (!result.field) {
      setErrors(result.errors);
      return;
    }
    onApply(result.field);
    setOpen(false);
  };

  const siblingListId = `${baseId}-siblings`;

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogTrigger asChild>
        <Button
          type="button"
          variant={hasAdvanced ? 'secondary' : 'ghost'}
          size="sm"
          className="h-7 w-7 p-0"
          aria-label={`Edit advanced schema for ${field.name || 'field'}`}
        >
          <Settings2 className="h-3.5 w-3.5" />
        </Button>
      </DialogTrigger>
      <DialogContent className="max-h-[85vh] max-w-3xl overflow-y-auto">
        <DialogHeader>
          <DialogTitle>
            Advanced Schema{field.name ? ` — ${field.name}` : ''}
          </DialogTitle>
          <DialogDescription>
            Display, validation, and structure metadata for this schema field.
          </DialogDescription>
        </DialogHeader>
        {draft && (
          <div className="space-y-5">
            <DialogSection title="Display">
              <div className="grid grid-cols-2 gap-3">
                <div className="space-y-1">
                  <Label htmlFor={`${baseId}-label`}>Label</Label>
                  <Input
                    id={`${baseId}-label`}
                    value={draft.label}
                    onChange={(e) => updateDraft({ label: e.target.value })}
                    placeholder="Display label"
                    disabled={readOnly}
                  />
                </div>
                <div className="space-y-1">
                  <Label htmlFor={`${baseId}-placeholder`}>Placeholder</Label>
                  <Input
                    id={`${baseId}-placeholder`}
                    value={draft.placeholder}
                    onChange={(e) =>
                      updateDraft({ placeholder: e.target.value })
                    }
                    placeholder="Placeholder text"
                    disabled={readOnly}
                  />
                </div>
                <div className="space-y-1">
                  <Label htmlFor={`${baseId}-order`}>Order</Label>
                  <Input
                    id={`${baseId}-order`}
                    type="number"
                    value={draft.order}
                    onChange={(e) => updateDraft({ order: e.target.value })}
                    placeholder="Sort order"
                    disabled={readOnly}
                  />
                  {errors.order && (
                    <p className="text-xs text-destructive">{errors.order}</p>
                  )}
                </div>
                <div className="space-y-1">
                  <Label id={`${baseId}-format-label`}>Format</Label>
                  <Select
                    value={draft.format || FORMAT_NONE}
                    onValueChange={(value) =>
                      updateDraft({
                        format: value === FORMAT_NONE ? '' : value,
                      })
                    }
                    disabled={readOnly}
                  >
                    <SelectTrigger aria-labelledby={`${baseId}-format-label`}>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value={FORMAT_NONE}>None</SelectItem>
                      {formatOptions.map((format) => (
                        <SelectItem key={format} value={format}>
                          {format}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
              </div>
            </DialogSection>

            <DialogSection title="Validation">
              <div className="grid grid-cols-2 gap-3">
                <div className="space-y-1">
                  <Label htmlFor={`${baseId}-min`}>Min</Label>
                  <Input
                    id={`${baseId}-min`}
                    type="number"
                    value={draft.min}
                    onChange={(e) => updateDraft({ min: e.target.value })}
                    placeholder="Min value / length"
                    disabled={readOnly}
                  />
                  {errors.min && (
                    <p className="text-xs text-destructive">{errors.min}</p>
                  )}
                </div>
                <div className="space-y-1">
                  <Label htmlFor={`${baseId}-max`}>Max</Label>
                  <Input
                    id={`${baseId}-max`}
                    type="number"
                    value={draft.max}
                    onChange={(e) => updateDraft({ max: e.target.value })}
                    placeholder="Max value / length"
                    disabled={readOnly}
                  />
                  {errors.max && (
                    <p className="text-xs text-destructive">{errors.max}</p>
                  )}
                </div>
                <div className="col-span-2 space-y-1">
                  <Label htmlFor={`${baseId}-pattern`}>Pattern</Label>
                  <Input
                    id={`${baseId}-pattern`}
                    value={draft.pattern}
                    onChange={(e) => updateDraft({ pattern: e.target.value })}
                    placeholder="^[a-z]+$"
                    disabled={readOnly}
                    className="font-mono"
                    spellCheck={false}
                  />
                  {patternWarning && (
                    <p className="text-xs text-amber-600">{patternWarning}</p>
                  )}
                </div>
                <div className="col-span-2 space-y-1">
                  <Label htmlFor={`${baseId}-example`}>Example (JSON)</Label>
                  <Input
                    id={`${baseId}-example`}
                    value={draft.example}
                    onChange={(e) => updateDraft({ example: e.target.value })}
                    placeholder='"sample" or {"key": 1}'
                    disabled={readOnly}
                    className="font-mono"
                    spellCheck={false}
                  />
                  {errors.example && (
                    <p className="text-xs text-destructive">{errors.example}</p>
                  )}
                </div>
              </div>
            </DialogSection>

            {fieldType === 'array' && (
              <DialogSection
                title="Array items"
                hint="Definition of each element in this array field."
              >
                <div className="w-64 space-y-1">
                  <Label id={`${baseId}-items-type-label`}>Element type</Label>
                  <Select
                    value={draft.itemsType}
                    onValueChange={(value) => updateDraft({ itemsType: value })}
                    disabled={readOnly}
                  >
                    <SelectTrigger
                      aria-labelledby={`${baseId}-items-type-label`}
                    >
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {SUPPORTED_TYPES.map((type) => (
                        <SelectItem key={type.value} value={type.value}>
                          {type.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
                {draft.itemsType === 'object' && (
                  <NestedFieldsEditor
                    label="Element properties"
                    fields={draft.itemsProperties}
                    onChange={(nested) =>
                      updateDraft({ itemsProperties: nested })
                    }
                    readOnly={readOnly}
                    showEnum={showEnum}
                  />
                )}
              </DialogSection>
            )}

            {(fieldType === 'object' || draft.properties.length > 0) && (
              <DialogSection
                title="Object properties"
                hint="Nested fields of this object."
              >
                <NestedFieldsEditor
                  label="Properties"
                  fields={draft.properties}
                  onChange={(nested) => updateDraft({ properties: nested })}
                  readOnly={readOnly}
                  showEnum={showEnum}
                />
              </DialogSection>
            )}

            <DialogSection
              title="Conditional visibility"
              hint="Show this field only when a sibling field matches a value."
            >
              {draft.visibleWhenRows.length === 0 ? (
                !readOnly && (
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={addVisibleWhenRow}
                  >
                    <Plus className="mr-2 h-4 w-4" />
                    Add visibility rule
                  </Button>
                )
              ) : (
                <div className="space-y-2">
                  <div className="w-64 space-y-1">
                    <Label htmlFor={`${baseId}-vw-field`}>Sibling field</Label>
                    <Input
                      id={`${baseId}-vw-field`}
                      list={siblingListId}
                      value={draft.visibleWhenField}
                      onChange={(e) =>
                        updateDraft({ visibleWhenField: e.target.value })
                      }
                      placeholder="fieldName"
                      disabled={readOnly}
                      className="font-mono"
                    />
                    <datalist id={siblingListId}>
                      {siblingNames.map((name) => (
                        <option key={name} value={name} />
                      ))}
                    </datalist>
                  </div>
                  {draft.visibleWhenRows.map((row, rowIndex) => {
                    const otherOperators = new Set(
                      draft.visibleWhenRows
                        .filter((_, i) => i !== rowIndex)
                        .map((other) => other.operator)
                    );
                    return (
                      <div key={rowIndex} className="flex items-center gap-2">
                        <Select
                          value={row.operator}
                          onValueChange={(value) => {
                            const rows = [...draft.visibleWhenRows];
                            rows[rowIndex] = {
                              ...rows[rowIndex],
                              operator: value as VisibleWhenOperator,
                            };
                            updateDraft({ visibleWhenRows: rows });
                          }}
                          disabled={readOnly}
                        >
                          <SelectTrigger className="w-36">
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            <SelectItem
                              value="equals"
                              disabled={otherOperators.has('equals')}
                            >
                              equals
                            </SelectItem>
                            <SelectItem
                              value="notEquals"
                              disabled={otherOperators.has('notEquals')}
                            >
                              not equals
                            </SelectItem>
                          </SelectContent>
                        </Select>
                        <Input
                          value={row.value}
                          onChange={(e) => {
                            const rows = [...draft.visibleWhenRows];
                            rows[rowIndex] = {
                              ...rows[rowIndex],
                              value: e.target.value,
                            };
                            updateDraft({ visibleWhenRows: rows });
                          }}
                          placeholder='manual, true, 5, "quoted string"'
                          disabled={readOnly}
                          className="flex-1 font-mono"
                          aria-label={`Visibility ${
                            row.operator === 'equals' ? 'equals' : 'not equals'
                          } value`}
                        />
                        {!readOnly && (
                          <Button
                            type="button"
                            variant="ghost"
                            size="sm"
                            className="h-7 w-7 p-0"
                            aria-label="Remove visibility condition"
                            onClick={() =>
                              updateDraft({
                                visibleWhenRows: draft.visibleWhenRows.filter(
                                  (_, i) => i !== rowIndex
                                ),
                              })
                            }
                          >
                            <Trash2 className="h-3 w-3" />
                          </Button>
                        )}
                      </div>
                    );
                  })}
                  {!readOnly && draft.visibleWhenRows.length < 2 && (
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      onClick={addVisibleWhenRow}
                    >
                      <Plus className="mr-2 h-4 w-4" />
                      Add condition
                    </Button>
                  )}
                </div>
              )}
              {errors.visibleWhen && (
                <p className="text-sm text-destructive">{errors.visibleWhen}</p>
              )}
            </DialogSection>

            <details className="rounded-md border">
              <summary className="cursor-pointer px-3 py-2 text-sm font-medium">
                Unknown extensions (JSON)
                {extensionCount > 0 ? ` (${extensionCount})` : ''}
              </summary>
              <div className="space-y-2 border-t p-3">
                <p className="text-xs text-muted-foreground">
                  Unrecognized keys preserved verbatim on this field. Known
                  schema keys must be edited with the controls above.
                </p>
                <Textarea
                  value={draft.extensionsText}
                  onChange={(e) =>
                    updateDraft({ extensionsText: e.target.value })
                  }
                  disabled={readOnly}
                  className="min-h-[120px] font-mono text-xs"
                  spellCheck={false}
                  placeholder='{"x-custom": "value"}'
                  aria-label="Unknown extensions (JSON)"
                />
                {errors.extensions && (
                  <p className="text-sm text-destructive">{errors.extensions}</p>
                )}
              </div>
            </details>
          </div>
        )}
        <DialogFooter>
          <Button
            type="button"
            variant="outline"
            onClick={() => setOpen(false)}
          >
            Cancel
          </Button>
          <Button type="button" onClick={handleApply} disabled={readOnly}>
            Apply
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

interface SchemaFieldsEditorProps {
  label: string;
  fields: SchemaField[];
  onChange: (fields: SchemaField[]) => void;
  readOnly?: boolean;
  emptyMessage?: string;
  hideLabel?: boolean;
  showEnum?: boolean;
}

export function SchemaFieldsEditor({
  label,
  fields,
  onChange,
  readOnly = false,
  emptyMessage = 'No fields defined.',
  hideLabel = false,
  showEnum = false,
}: SchemaFieldsEditorProps) {
  const [schemaFieldValidationErrors, setSchemaFieldValidationErrors] =
    useState<RustSchemaFieldsValidationError[]>([]);

  useEffect(() => {
    let cancelled = false;

    validateSchemaFieldsWithRust(label, fields).then((result) => {
      if (cancelled) {
        return;
      }

      setSchemaFieldValidationErrors(
        result.status === 'invalid' ? result.schemaErrors : []
      );
    });

    return () => {
      cancelled = true;
    };
  }, [fields, label]);

  const fieldNameErrorsByIndex = useMemo(() => {
    const errorsByIndex = new Map<number, string>();

    for (const error of schemaFieldValidationErrors) {
      if (error.code !== 'E008') {
        continue;
      }

      for (const rowIndex of error.rowIndices) {
        errorsByIndex.set(rowIndex, 'Field name must be unique.');
      }
    }

    return errorsByIndex;
  }, [schemaFieldValidationErrors]);
  const errorIdPrefix = useMemo(
    () =>
      `${label.toLowerCase().replace(/[^a-z0-9]+/g, '-') || 'schema'}-field`,
    [label]
  );

  const handleAdd = () => {
    onChange([
      ...fields,
      { name: '', type: 'string', required: true, description: '' },
    ]);
  };

  const handleRemove = (index: number) => {
    const newFields = [...fields];
    newFields.splice(index, 1);
    onChange(newFields);
  };

  const handleChange = (
    index: number,
    field: keyof SchemaField,
    value: any
  ) => {
    const newFields = [...fields];
    newFields[index] = { ...newFields[index], [field]: value };
    onChange(newFields);
  };

  return (
    <div className="space-y-2">
      {!hideLabel && <Label className="text-sm font-medium">{label}</Label>}
      <div className="border rounded-lg overflow-x-auto">
        <table className="w-full min-w-[980px]">
          <thead>
            <tr className="border-b">
              <th className="text-left p-2 text-sm font-medium text-muted-foreground">
                Field Name
              </th>
              <th className="text-left p-2 text-sm font-medium text-muted-foreground">
                Type
              </th>
              <th className="w-20 text-center p-2 text-sm font-medium text-muted-foreground">
                Required
              </th>
              <th className="w-20 text-center p-2 text-sm font-medium text-muted-foreground">
                Nullable
              </th>
              <th className="text-left p-2 text-sm font-medium text-muted-foreground">
                Description
              </th>
              <th className="text-left p-2 text-sm font-medium text-muted-foreground">
                Default
              </th>
              <th className="text-left p-2 text-sm font-medium text-muted-foreground">
                Format
              </th>
              {showEnum && (
                <th className="text-left p-2 text-sm font-medium text-muted-foreground">
                  Enum
                </th>
              )}
              <th className="w-20 text-center p-2 text-sm font-medium text-muted-foreground">
                Advanced
              </th>
              {!readOnly && (
                <th className="w-16 text-center p-2 text-sm font-medium text-muted-foreground">
                  Actions
                </th>
              )}
            </tr>
          </thead>
          <tbody>
            {fields.map((field, index) => {
              const fieldNameError = fieldNameErrorsByIndex.get(index) ?? null;
              const fieldNameErrorId = fieldNameError
                ? `${errorIdPrefix}-${index}-name-error`
                : undefined;

              return (
                <tr key={index} className="border-b hover:bg-muted/30">
                  <td className="p-2 align-top">
                    <Input
                      value={field.name}
                      onChange={(e) =>
                        handleChange(index, 'name', e.target.value)
                      }
                      placeholder="fieldName"
                      disabled={readOnly}
                      aria-invalid={!!fieldNameError}
                      aria-describedby={fieldNameErrorId}
                      className={cn(
                        'font-mono text-sm border-0 p-1 h-auto focus-visible:ring-0 focus-visible:ring-offset-0',
                        fieldNameError &&
                          'bg-destructive/10 text-destructive focus-visible:ring-destructive'
                      )}
                    />
                    {fieldNameError && (
                      <p
                        id={fieldNameErrorId}
                        className="mt-1 text-xs text-destructive"
                      >
                        {fieldNameError}
                      </p>
                    )}
                  </td>
                  <td className="p-2 align-top">
                    <Select
                      value={field.type || 'string'}
                      onValueChange={(value) =>
                        handleChange(index, 'type', value)
                      }
                      disabled={readOnly}
                    >
                      <SelectTrigger className="h-7 border-0 focus:ring-0 focus:ring-offset-0">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {SUPPORTED_TYPES.map((type) => (
                          <SelectItem key={type.value} value={type.value}>
                            {type.label}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </td>
                  <td className="w-20 text-center p-2 align-top">
                    <Checkbox
                      checked={field.required}
                      onCheckedChange={(checked) =>
                        handleChange(index, 'required', !!checked)
                      }
                      disabled={readOnly}
                    />
                  </td>
                  <td className="w-20 text-center p-2 align-top">
                    <Checkbox
                      checked={!!field.nullable}
                      onCheckedChange={(checked) =>
                        handleChange(index, 'nullable', !!checked)
                      }
                      disabled={readOnly}
                    />
                  </td>
                  <td className="p-2 align-top">
                    <Input
                      value={field.description}
                      onChange={(e) =>
                        handleChange(index, 'description', e.target.value)
                      }
                      placeholder="Field description"
                      disabled={readOnly}
                      className="text-sm border-0 p-1 h-auto focus-visible:ring-0 focus-visible:ring-offset-0"
                    />
                  </td>
                  <td className="p-2 align-top">
                    <Input
                      value={formatFieldValue(field.defaultValue)}
                      onChange={(e) =>
                        handleChange(
                          index,
                          'defaultValue',
                          parseDefaultValue(e.target.value, field.type)
                        )
                      }
                      placeholder="Default"
                      disabled={readOnly}
                      className="font-mono text-sm border-0 p-1 h-auto focus-visible:ring-0 focus-visible:ring-offset-0"
                    />
                  </td>
                  <td className="p-2 align-top">
                    <Input
                      value={field.format || ''}
                      onChange={(e) =>
                        handleChange(
                          index,
                          'format',
                          e.target.value || undefined
                        )
                      }
                      placeholder="date, email..."
                      disabled={readOnly}
                      className="font-mono text-sm border-0 p-1 h-auto focus-visible:ring-0 focus-visible:ring-offset-0"
                    />
                  </td>
                  {showEnum && (
                    <td className="p-2 align-top">
                      <Input
                        value={formatEnumValue(field.enum)}
                        onChange={(e) => {
                          const newFields = [...fields];
                          newFields[index] = {
                            ...newFields[index],
                            enum: parseEnumValue(e.target.value),
                          };
                          onChange(newFields);
                        }}
                        placeholder="val1, val2, ..."
                        disabled={readOnly}
                        className="font-mono text-sm border-0 p-1 h-auto focus-visible:ring-0 focus-visible:ring-offset-0"
                      />
                    </td>
                  )}
                  <td className="w-20 text-center p-2 align-top">
                    <AdvancedSchemaFieldDialog
                      field={field}
                      readOnly={readOnly}
                      showEnum={showEnum}
                      siblingNames={fields
                        .filter((_, otherIndex) => otherIndex !== index)
                        .map((other) => other.name)
                        .filter(Boolean)}
                      onApply={(updatedField) => {
                        const newFields = [...fields];
                        newFields[index] = updatedField;
                        onChange(newFields);
                      }}
                    />
                  </td>
                  {!readOnly && (
                    <td className="w-16 text-center p-2 align-top">
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
              );
            })}
            {fields.length === 0 && (
              <tr>
                <td
                  colSpan={(readOnly ? 8 : 9) + (showEnum ? 1 : 0)}
                  className="p-4 text-center text-sm text-muted-foreground"
                >
                  {emptyMessage}
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
          Add Field
        </Button>
      )}
    </div>
  );
}
