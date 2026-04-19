export type VisibleWhen = {
  field: string;
  equals?: any;
  notEquals?: any;
};

export type SchemaField = {
  name: string;
  type?: string;
  required?: boolean;
  description?: string;
  defaultValue?: any;
  enum?: string[];
  // Form rendering extensions
  label?: string;
  placeholder?: string;
  order?: number;
  format?: string;
  min?: number;
  max?: number;
  pattern?: string;
  properties?: SchemaField[];
  visibleWhen?: VisibleWhen;
};

function safeParseSchema(raw: any): any {
  if (raw === undefined || raw === null) {
    return null;
  }

  if (typeof raw === 'string') {
    try {
      return JSON.parse(raw);
    } catch {
      return null;
    }
  }

  return raw;
}

/**
 * Infer a format from the field name when no explicit format is provided.
 * E.g. "follow_up_datetime" → "datetime", "delivery_date" → "date".
 */
function inferFormatFromName(name: string): string | undefined {
  const lower = name.toLowerCase();
  // Check datetime before date so "follow_up_datetime" doesn't match "date" first
  if (
    lower.endsWith('_datetime') ||
    lower.endsWith('_date_time') ||
    lower === 'datetime'
  ) {
    return 'datetime';
  }
  if (lower.endsWith('_date') || lower === 'date') {
    return 'date';
  }
  return undefined;
}

/** Extract form rendering extensions from a raw field object. */
function extractExtensions(field: Record<string, any>): Partial<SchemaField> {
  const ext: Partial<SchemaField> = {};
  if (field.label) ext.label = field.label;
  if (field.placeholder) ext.placeholder = field.placeholder;
  if (field.order != null) ext.order = Number(field.order);
  if (field.format) ext.format = field.format;
  if (field.min != null) ext.min = Number(field.min);
  if (field.max != null) ext.max = Number(field.max);
  if (field.pattern) ext.pattern = field.pattern;
  if (field.visibleWhen) ext.visibleWhen = field.visibleWhen;
  if (field.visible_when) {
    ext.visibleWhen = {
      field: field.visible_when.field,
      equals: field.visible_when.equals,
      notEquals: field.visible_when.not_equals ?? field.visible_when.notEquals,
    };
  }
  if (field.properties && typeof field.properties === 'object') {
    ext.properties = parseSchema(field.properties);
  }
  return ext;
}

export function parseSchema(raw: any): SchemaField[] {
  const schema = safeParseSchema(raw);

  if (!schema || typeof schema !== 'object') {
    return [];
  }

  let fields: SchemaField[];

  // Handle JSON schema style { properties, required }
  if ('properties' in schema) {
    const required = Array.isArray((schema as any).required)
      ? ((schema as any).required as string[])
      : [];
    const properties = (schema as any).properties || {};

    fields = Object.entries(properties).map(([name, value]) => {
      const field = value as Record<string, any>;
      return {
        name,
        type: field.type ?? 'string',
        required: required.includes(name) || !!field.required,
        description: field.description,
        defaultValue: field.default,
        ...(Array.isArray(field.enum) && field.enum.length > 0
          ? { enum: field.enum.map(String) }
          : {}),
        ...extractExtensions(field),
      };
    });
  } else {
    // Handle simple map-based schema { fieldName: { type, required, ... } }
    fields = Object.entries(schema).map(([name, value]) => {
      const field = (value as Record<string, any>) || {};
      return {
        name,
        type: field.type ?? 'string',
        required: !!field.required,
        description: field.description,
        defaultValue: field.default,
        ...(Array.isArray(field.enum) && field.enum.length > 0
          ? { enum: field.enum.map(String) }
          : {}),
        ...extractExtensions(field),
      };
    });
  }

  // Infer format from field name when type is string and no explicit format
  for (const field of fields) {
    if (field.type === 'string' && !field.format) {
      const inferred = inferFormatFromName(field.name);
      if (inferred) field.format = inferred;
    }
  }

  // Sort by order if any field has it, otherwise preserve original order
  if (fields.some((f) => f.order != null)) {
    fields.sort((a, b) => (a.order ?? 9999) - (b.order ?? 9999));
  }

  return fields;
}

export function buildSchemaFromFields(
  fields: SchemaField[]
): Record<string, any> {
  return fields.reduce<Record<string, any>>((acc, field) => {
    if (!field.name) {
      return acc;
    }

    const fieldType = field.type || 'string';
    const schemaField: Record<string, any> = {
      type: fieldType,
      required: field.required !== undefined ? field.required : true,
    };

    // For array types, add a default items definition
    if (fieldType === 'array') {
      schemaField.items = { type: 'string' };
    }

    if (field.description) {
      schemaField.description = field.description;
    }

    if (Array.isArray(field.enum) && field.enum.length > 0) {
      schemaField.enum = field.enum;
    }

    if (field.defaultValue !== undefined && field.defaultValue !== '') {
      schemaField.default = field.defaultValue;
    }

    // Form rendering extensions
    if (field.label) schemaField.label = field.label;
    if (field.placeholder) schemaField.placeholder = field.placeholder;
    if (field.order != null) schemaField.order = field.order;
    if (field.format) schemaField.format = field.format;
    if (field.min != null) schemaField.min = field.min;
    if (field.max != null) schemaField.max = field.max;
    if (field.pattern) schemaField.pattern = field.pattern;
    if (field.visibleWhen) schemaField.visibleWhen = field.visibleWhen;
    if (field.properties && field.properties.length > 0) {
      schemaField.properties = buildSchemaFromFields(field.properties);
    }

    acc[field.name] = schemaField;
    return acc;
  }, {});
}

export function inferSchemaFromMapping(
  mappings: { type?: string | null | undefined }[]
): SchemaField[] {
  if (!Array.isArray(mappings)) {
    return [];
  }

  return mappings
    .map((mapping) => mapping?.type)
    .filter((name): name is string => !!name)
    .map((name) => ({
      name,
      type: 'string',
      required: true,
    }));
}
