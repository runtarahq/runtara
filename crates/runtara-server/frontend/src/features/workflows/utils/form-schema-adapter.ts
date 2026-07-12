import type { FormDefinition, FormField } from '@/shared/forms';

import type { SchemaField } from './schema';

const FIELD_TYPES = new Set([
  'string',
  'integer',
  'number',
  'boolean',
  'array',
  'object',
  'file',
]);

function visibleCondition(field: SchemaField): unknown | undefined {
  if (!field.visibleWhen?.field) return undefined;
  const clauses: unknown[] = [];
  const reference = {
    type: 'value',
    valueType: 'reference',
    value: field.visibleWhen.field,
  };
  if (field.visibleWhen.equals !== undefined) {
    clauses.push({
      type: 'operation',
      op: 'EQ',
      arguments: [
        reference,
        {
          type: 'value',
          valueType: 'immediate',
          value: field.visibleWhen.equals,
        },
      ],
    });
  }
  if (field.visibleWhen.notEquals !== undefined) {
    clauses.push({
      type: 'operation',
      op: 'NE',
      arguments: [
        reference,
        {
          type: 'value',
          valueType: 'immediate',
          value: field.visibleWhen.notEquals,
        },
      ],
    });
  }
  if (clauses.length === 0) return undefined;
  return clauses.length === 1
    ? clauses[0]
    : { type: 'operation', op: 'AND', arguments: clauses };
}

function adaptField(field: SchemaField): FormField {
  const type = FIELD_TYPES.has(field.type ?? '') ? field.type! : 'string';
  return {
    type: type as FormField['type'],
    required: field.required !== false,
    description: field.description,
    default: field.defaultValue,
    enum: field.enum,
    example: field.example,
    nullable: field.nullable,
    label: field.label,
    placeholder: field.placeholder,
    order: field.order,
    format: field.format,
    min: field.min,
    max: field.max,
    pattern: field.pattern,
    items:
      field.items && typeof field.items === 'object'
        ? adaptField({ ...(field.items as SchemaField), name: 'item' })
        : type === 'array'
          ? { type: 'string' }
          : undefined,
    properties: field.properties
      ? Object.fromEntries(
          field.properties.map((property) => [
            property.name,
            adaptField(property),
          ])
        )
      : undefined,
    conditions: { visible: visibleCondition(field) },
  };
}

export function workflowSchemaToFormDefinition(
  fields: SchemaField[]
): FormDefinition {
  return {
    schemaVersion: 1,
    allowUnknownFields: false,
    fields: Object.fromEntries(
      fields.map((field) => [field.name, adaptField(field)])
    ),
  };
}

export function initialWorkflowFormValues(
  definition: FormDefinition
): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(definition.fields).map(([name, field]) => [
      name,
      field.default !== undefined
        ? field.default
        : field.type === 'boolean'
          ? false
          : field.type === 'array'
            ? []
            : field.type === 'object'
              ? {}
              : '',
    ])
  );
}
