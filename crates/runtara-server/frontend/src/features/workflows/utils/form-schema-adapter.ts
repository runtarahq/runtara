import { useEffect, useMemo, useState } from 'react';

import {
  ensureRustValidationInitialized,
  normalizeSchemaFieldsFormJson,
} from '@/shared/lib/rust-validation-wasm';
import type { FormDefinition, FormField } from '@/shared/forms';

const EMPTY_FORM: FormDefinition = {
  schemaVersion: 1,
  fields: {},
  sections: [],
  allowUnknownFields: false,
};

function normalizeWireField(field: unknown): Record<string, unknown> {
  if (!field || typeof field !== 'object' || Array.isArray(field)) {
    return { type: 'string' };
  }
  const source = { ...(field as Record<string, unknown>) };
  if ('defaultValue' in source && !('default' in source)) {
    source.default = source.defaultValue;
  }
  delete source.defaultValue;
  if (Array.isArray(source.properties)) {
    source.properties = Object.fromEntries(
      source.properties
        .filter(
          (property): property is Record<string, unknown> =>
            Boolean(property) &&
            typeof property === 'object' &&
            typeof (property as Record<string, unknown>).name === 'string'
        )
        .map((property) => {
          const { name, ...nested } = property;
          return [String(name), normalizeWireField(nested)];
        })
    );
  }
  return source;
}

/**
 * Preserve accepted workflow schema wire envelopes while leaving field
 * semantics and condition normalization entirely to Rust.
 */
export function workflowSchemaWireMap(raw: unknown): Record<string, unknown> {
  let schema = raw;
  if (typeof schema === 'string') {
    try {
      schema = JSON.parse(schema);
    } catch {
      return {};
    }
  }
  if (!schema || typeof schema !== 'object' || Array.isArray(schema)) return {};
  const object = schema as Record<string, unknown>;

  if (object.properties && typeof object.properties === 'object') {
    const required = new Set(
      Array.isArray(object.required) ? object.required.map(String) : []
    );
    return Object.fromEntries(
      Object.entries(object.properties as Record<string, unknown>).map(
        ([name, field]) => [
          name,
          { ...normalizeWireField(field), required: required.has(name) },
        ]
      )
    );
  }

  return Object.fromEntries(
    Object.entries(object).map(([name, field]) => [
      name,
      normalizeWireField(field),
    ])
  );
}

export async function normalizeWorkflowFormDefinition(
  rawSchema: unknown
): Promise<FormDefinition> {
  const schema = workflowSchemaWireMap(rawSchema);
  if (Object.keys(schema).length === 0) return EMPTY_FORM;
  await ensureRustValidationInitialized();
  const response = JSON.parse(
    normalizeSchemaFieldsFormJson(JSON.stringify(schema))
  ) as { success?: boolean; definition?: FormDefinition; error?: string };
  if (!response.success || !response.definition) {
    throw new Error(response.error ?? 'Workflow form normalization failed');
  }
  return response.definition;
}

export function useWorkflowFormDefinition(rawSchema: unknown): {
  definition: FormDefinition;
  loading: boolean;
  error: string | null;
} {
  const schemaJson = JSON.stringify(rawSchema ?? {});
  const schemaSnapshot = useMemo(() => JSON.parse(schemaJson), [schemaJson]);
  const [state, setState] = useState<{
    definition: FormDefinition;
    loading: boolean;
    error: string | null;
  }>({ definition: EMPTY_FORM, loading: true, error: null });

  useEffect(() => {
    let current = true;
    setState({ definition: EMPTY_FORM, loading: true, error: null });
    void normalizeWorkflowFormDefinition(schemaSnapshot)
      .then((definition) => {
        if (current) setState({ definition, loading: false, error: null });
      })
      .catch((error: unknown) => {
        if (!current) return;
        setState({
          definition: EMPTY_FORM,
          loading: false,
          error:
            error instanceof Error
              ? error.message
              : 'Workflow form normalization failed',
        });
      });
    return () => {
      current = false;
    };
  }, [schemaSnapshot]);

  return state;
}

export function initialWorkflowFormValues(
  definition: FormDefinition
): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(definition.fields).map(([name, field]) => [
      name,
      field.default !== undefined ? field.default : initialValueForField(field),
    ])
  );
}

function initialValueForField(field: FormField): unknown {
  if (field.type === 'boolean') return false;
  if (field.type === 'array') return [];
  if (field.type === 'object') return {};
  return '';
}
