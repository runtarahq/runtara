/**
 * Authoring-time resolution of a reference path to its display metadata and
 * (when statically known) its type.
 *
 * Suggestion entries carry a type in the variable picker, but historically it
 * was discarded the moment a reference was inserted — the pill fell back to
 * guessing an icon from path substrings ("price" → dollar), a confident-looking
 * but wrong signal. This module re-resolves the stored path against the same
 * data the picker used (previous steps, workflow input schema, variables), so
 * a used reference shows its real type — or honestly nothing when the shape is
 * runtime-dependent.
 */
import { SchemaField } from '../EditorSidebar/SchemaFieldsEditor';
import { SimpleVariable } from './NodeFormContext';
import { StepInfo, StepParameter } from './shared';

export interface ParsedStepReference {
  stepId: string;
  /** Path remainder after the step segment, e.g. "outputs.items" or "stats". */
  rest: string;
}

/**
 * Parses both spellings of a step reference: `steps['id'].rest` (what the
 * picker inserts) and `steps.id.rest` (hand-written / imported graphs).
 */
export function parseStepReference(path: string): ParsedStepReference | null {
  let match = path.match(/^steps\['([^']+)'\]\.?(.*)$/);
  if (match) {
    return { stepId: match[1], rest: match[2] ?? '' };
  }
  match = path.match(/^steps\.([^.[]+)\.?(.*)$/);
  if (match) {
    return { stepId: match[1], rest: match[2] ?? '' };
  }
  return null;
}

export interface StepReferenceDisplay {
  stepName?: string;
  /**
   * Short field path for pill display: "items" for outputs fields, "outputs"
   * for the whole outputs value, sibling names ("stats") as-is.
   */
  fieldPath?: string;
}

/**
 * Friendly display info for a step reference, for both path spellings.
 * Returns {} when the path is not a step reference or the step is unknown.
 */
export function describeStepReference(
  path: string,
  previousSteps: StepInfo[]
): StepReferenceDisplay {
  const parsed = parseStepReference(path);
  if (!parsed) {
    return {};
  }
  const step = previousSteps.find((s) => s.id === parsed.stepId);
  if (!step) {
    return {};
  }
  let fieldPath = parsed.rest;
  if (fieldPath.startsWith('outputs.')) {
    fieldPath = fieldPath.slice('outputs.'.length);
  }
  return { stepName: step.name, fieldPath: fieldPath || 'outputs' };
}

/** Built-in runtime variables and their types (undefined = runtime-shaped). */
const BUILTIN_VARIABLE_TYPES: Record<string, string | undefined> = {
  _workflow_id: 'string',
  _instance_id: 'string',
  _tenant_id: 'string',
  _signal_id: 'string',
  _index: 'integer',
  _item: undefined,
  _loop: 'object',
  _loop_indices: 'array',
};

function flattenParameters(parameters: StepParameter[]): StepParameter[] {
  const result: StepParameter[] = [];
  for (const parameter of parameters) {
    result.push(parameter);
    if (parameter.children && parameter.children.length > 0) {
      result.push(...flattenParameters(parameter.children));
    }
  }
  return result;
}

function resolveStepReferenceType(
  path: string,
  previousSteps: StepInfo[]
): string | undefined {
  const parsed = parseStepReference(path);
  if (!parsed) {
    return undefined;
  }
  const step = previousSteps.find((s) => s.id === parsed.stepId);
  if (!step) {
    return undefined;
  }
  // Normalize to the bracket form used by suggestion paths.
  const normalized = `steps['${parsed.stepId}']${parsed.rest ? `.${parsed.rest}` : ''}`;
  const match = flattenParameters(step.outputs).find(
    (parameter) => parameter.path === normalized
  );
  return match?.type;
}

function resolveSchemaFieldType(
  segments: string[],
  fields: SchemaField[] | undefined
): string | undefined {
  if (!fields || segments.length === 0) {
    return undefined;
  }
  const [head, ...rest] = segments;
  const field = fields.find((f) => f.name === head);
  if (!field) {
    return undefined;
  }
  if (rest.length === 0) {
    return field.type?.toLowerCase() || undefined;
  }
  return resolveSchemaFieldType(rest, field.properties);
}

export interface ReferenceTypeContext {
  previousSteps?: StepInfo[];
  inputSchemaFields?: SchemaField[];
  variables?: SimpleVariable[];
}

/**
 * Resolves a reference path to its statically-known type
 * ("string" | "number" | "integer" | "boolean" | "array" | "object" | …), or
 * undefined when the type is unknown or runtime-dependent. Never guesses.
 */
export function resolveReferenceType(
  path: string,
  context: ReferenceTypeContext
): string | undefined {
  if (!path) {
    return undefined;
  }

  if (path.startsWith('steps.') || path.startsWith("steps['")) {
    return resolveStepReferenceType(path, context.previousSteps ?? []);
  }

  // Workflow input data: both spellings the editor teaches.
  const dataRest = stripPrefix(path, ['workflow.inputs.data', 'data']);
  if (dataRest !== null) {
    if (dataRest === '') {
      return 'object';
    }
    return resolveSchemaFieldType(
      dataRest.split('.'),
      context.inputSchemaFields
    );
  }

  const variableRest = stripPrefix(path, [
    'workflow.inputs.variables',
    'variables',
  ]);
  if (variableRest !== null && variableRest !== '') {
    const [name, ...tail] = variableRest.split('.');
    if (tail.length > 0) {
      return undefined;
    }
    if (name in BUILTIN_VARIABLE_TYPES) {
      return BUILTIN_VARIABLE_TYPES[name];
    }
    const variable = context.variables?.find((v) => v.name === name);
    return variable?.type?.toLowerCase() || undefined;
  }

  if (path === 'loop.index') {
    return 'integer';
  }
  if (path === 'loop') {
    return 'object';
  }

  return undefined;
}

const TYPE_ALIASES: Record<string, string> = {
  text: 'string',
  str: 'string',
  textarea: 'string',
  // Enum inputs surface as the 'select' widget type; on the wire they are
  // strings. Without this alias every reference into an enum field warned
  // "expects select".
  select: 'string',
  int: 'integer',
  double: 'number',
  float: 'number',
  bool: 'boolean',
  json: 'any',
  unknown: 'any',
};

/** Normalizes editor/schema type spellings to canonical JSON type names. */
export function normalizeTypeName(type?: string): string | undefined {
  if (!type) {
    return undefined;
  }
  const lower = type.toLowerCase();
  if (
    lower.startsWith('array<') ||
    lower.startsWith('[') ||
    lower.includes('[]')
  ) {
    return 'array';
  }
  return TYPE_ALIASES[lower] ?? lower;
}

export interface MismatchOptions {
  /**
   * The consumer coerces scalar values into strings at runtime (e.g. Finish
   * outputs with a "string" type hint always stringify numbers/booleans) —
   * scalar→string is then a supported pattern, not a mismatch.
   */
  scalarsCoerceToString?: boolean;
}

/**
 * Returns a human-readable warning when a resolved reference type cannot fit
 * the target field's declared type, or null when compatible / unknowable.
 * Advisory only — runtime coercion sometimes saves a mismatch, so this warns
 * rather than blocks (server-side E023 covers immediate values).
 */
export function referenceTypeMismatch(
  referenceType: string | undefined,
  fieldType: string | undefined,
  options: MismatchOptions = {}
): string | null {
  const reference = normalizeTypeName(referenceType);
  const field = normalizeTypeName(fieldType);
  if (!reference || !field) {
    return null;
  }
  // Unknowable or catch-all targets accept anything.
  if (
    reference === 'any' ||
    field === 'any' ||
    field === 'file' ||
    reference === 'null'
  ) {
    return null;
  }
  if (reference === field) {
    return null;
  }
  // An integer always fits a number-typed field.
  if (reference === 'integer' && field === 'number') {
    return null;
  }
  if (
    options.scalarsCoerceToString &&
    field === 'string' &&
    (reference === 'integer' || reference === 'number' || reference === 'boolean')
  ) {
    return null;
  }
  return `Reference is ${reference}; this field expects ${field}`;
}

/**
 * If `path` is `prefix` or starts with `prefix.`, returns the remainder
 * (possibly ''); otherwise null. Tries prefixes in order.
 */
function stripPrefix(path: string, prefixes: string[]): string | null {
  for (const prefix of prefixes) {
    if (path === prefix) {
      return '';
    }
    if (path.startsWith(`${prefix}.`)) {
      return path.slice(prefix.length + 1);
    }
  }
  return null;
}
