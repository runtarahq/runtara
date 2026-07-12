import type { ConnectionTypeDto } from '@/generated/RuntaraRuntimeApi';
import type { FormDefinition, FormField } from '@/shared/forms';

export type ConnectionTypeWithForm = ConnectionTypeDto & {
  formDefinition?: FormDefinition;
};

export type EditProjection = {
  values?: Record<string, unknown>;
  secretState?: Record<string, { configured: boolean; clearable: boolean }>;
  version?: string;
};

export type ConnectionSecretStateMap = NonNullable<
  EditProjection['secretState']
>;

export interface ConnectionParameterPatchValues {
  set: Record<string, unknown>;
  replaceSecrets: Record<string, string>;
  clear: string[];
}

function defaultForField(field: FormField): unknown {
  if (field.access === 'write') return '';
  if (field.default !== undefined) return field.default;
  if (field.type === 'boolean') return false;
  if (field.type === 'array') return [];
  if (field.type === 'object') return {};
  return '';
}

export function buildConnectionFormDefinition(
  connectionType: ConnectionTypeWithForm,
  mode: 'create' | 'edit',
  secretState: ConnectionSecretStateMap = {},
  clearedSecrets: ReadonlySet<string> = new Set()
): FormDefinition {
  const descriptor = connectionType.formDefinition ?? {
    schemaVersion: 1,
    fields: {},
    sections: [],
    allowUnknownFields: false,
  };
  const fields = Object.fromEntries(
    Object.entries(descriptor.fields).map(([name, field]) => [
      name,
      mode === 'edit' &&
      field.access === 'write' &&
      secretState[name]?.configured &&
      !clearedSecrets.has(name)
        ? {
            ...field,
            required: false,
            conditions: field.conditions
              ? { ...field.conditions, required: undefined }
              : undefined,
          }
        : field,
    ])
  );
  fields.title = {
    type: 'string',
    label: 'Title',
    description: 'A unique name to identify this connection',
    placeholder: 'Enter a descriptive name for this connection',
    required: true,
    order: -100,
    section: 'configuration',
  };
  const sections = descriptor.sections?.some(
    (section) => section.id === 'configuration'
  )
    ? descriptor.sections
    : [
        { id: 'configuration', label: 'Connection details', order: 0 },
        ...(descriptor.sections ?? []),
      ];
  return { ...descriptor, fields, sections };
}

export function buildConnectionParameterValues(
  definition: FormDefinition,
  initValues: Record<string, unknown> | undefined,
  mode: 'create' | 'edit'
): Record<string, unknown> {
  const projection = initValues?.editProjection as EditProjection | undefined;
  return Object.fromEntries(
    Object.entries(definition.fields).map(([name, field]) => {
      if (name === 'title') return [name, initValues?.title ?? ''];
      if (mode === 'edit' && projection?.values && name in projection.values) {
        return [name, projection.values[name]];
      }
      return [name, defaultForField(field)];
    })
  );
}

export function buildConnectionParameterPatch(
  definition: FormDefinition,
  parameters: Record<string, unknown>,
  projection: EditProjection,
  explicitSecretClears: readonly string[]
): ConnectionParameterPatchValues {
  const set: Record<string, unknown> = {};
  const replaceSecrets: Record<string, string> = {};
  const clear = new Set(explicitSecretClears);

  for (const [name, field] of Object.entries(definition.fields)) {
    const value = parameters[name];
    if (field.access === 'read') continue;
    if (field.access === 'write' || field.secret) {
      if (!clear.has(name) && typeof value === 'string' && value.length > 0) {
        replaceSecrets[name] = value;
      }
      continue;
    }

    const previous = projection.values?.[name];
    if (JSON.stringify(value) === JSON.stringify(previous)) continue;
    if (value === '' || value === null || value === undefined) {
      if (previous !== undefined) clear.add(name);
    } else {
      set[name] = value;
    }
  }

  return { set, replaceSecrets, clear: [...clear].sort() };
}
