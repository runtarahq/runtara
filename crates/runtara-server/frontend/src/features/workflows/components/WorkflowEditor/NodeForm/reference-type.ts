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
import { getStepOutputShape } from '@/features/workflows/utils/step-output-shapes';
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
  /**
   * Inside a Split body the DSL rebinds bare `data.*` to the current item —
   * the workflow-level input schema does not apply there. `data.*` resolves
   * against the Split's declared iteration schema when one exists
   * (splitItemSchemaFields), and to unknown otherwise.
   */
  insideSplitScope?: boolean;
  /** Declared iteration schema of the enclosing Split. */
  splitItemSchemaFields?: SchemaField[];
  /**
   * Inside a WaitForSignal onWait subgraph, `data.*` is scoped to the onWait
   * graph's own (editor-unmodeled) schema — resolve to unknown and never flag.
   */
  insideWaitScope?: boolean;
  /**
   * Id of the step being edited. Self-references are demoted to a save-time
   * warning by the server (SelfReference), so the inline check skips them
   * instead of calling the step "not upstream".
   */
  currentStepId?: string;
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
    if (context.insideSplitScope) {
      // `data.*` is the Split's current item here, not the workflow input —
      // resolve against the Split's declared iteration schema when present.
      const itemFields = context.splitItemSchemaFields;
      if (!itemFields || itemFields.length === 0) {
        return undefined;
      }
      if (dataRest === '') {
        return 'object';
      }
      return resolveSchemaFieldType(dataRest.split('.'), itemFields);
    }
    if (context.insideWaitScope) {
      // onWait scope's own schema is not modeled in the editor.
      return dataRest === '' ? 'object' : undefined;
    }
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

  if (path === 'iteration') {
    return 'object';
  }
  if (path === 'iteration.index') {
    return 'integer';
  }
  if (path === 'iteration.indices') {
    return 'array';
  }
  const iterationItemRest = stripPrefix(path, ['iteration.item']);
  if (iterationItemRest !== null) {
    if (iterationItemRest === '') {
      return context.insideSplitScope ? 'object' : undefined;
    }
    return context.insideSplitScope
      ? resolveSchemaFieldType(
          iterationItemRest.split('.'),
          context.splitItemSchemaFields
        )
      : undefined;
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
    (reference === 'integer' ||
      reference === 'number' ||
      reference === 'boolean')
  ) {
    return null;
  }
  return `Reference is ${reference}; this field expects ${field}`;
}

/**
 * Authoring-time existence check for a reference path. Returns a
 * human-readable error when the path provably cannot resolve — mirroring the
 * save-time validator's semantics (unknown upstream step; a named field on a
 * closed-shape output, E058; a named key into an array output, E059; an
 * undeclared workflow-input / Split-item field) — or null when the path is
 * valid or not statically checkable. Deliberately conservative: dynamic
 * shapes, bracket indexing, sibling fields, and variables are never flagged.
 */
export function validateReferencePath(
  path: string,
  context: ReferenceTypeContext
): string | null {
  if (!path) {
    return null;
  }

  if (path.startsWith('steps.') || path.startsWith("steps['")) {
    return validateStepReferencePath(
      path,
      context.previousSteps ?? [],
      context.currentStepId
    );
  }

  const dataRest = stripPrefix(path, ['workflow.inputs.data', 'data']);
  if (dataRest !== null && dataRest !== '') {
    // Bracket indexing (data.orders[0].sku) is normalized at runtime into
    // separate segments — mirror the step-reference branch and skip it here
    // rather than mismatching 'orders[0]' against the declared 'orders'.
    if (dataRest.includes('[')) {
      return null;
    }
    if (context.insideSplitScope) {
      // Bare data.* is the Split's current item; check its declared schema.
      // The explicit workflow.inputs.* spelling is left unchecked here.
      if (path.startsWith('workflow.inputs.')) {
        return null;
      }
      return validateSchemaFieldPath(
        dataRest.split('.'),
        context.splitItemSchemaFields,
        'the Split iteration schema'
      );
    }
    if (context.insideWaitScope) {
      // onWait scope's own schema is not modeled — never flag.
      return null;
    }
    return validateSchemaFieldPath(
      dataRest.split('.'),
      context.inputSchemaFields,
      'the workflow input schema'
    );
  }

  if (path.startsWith('iteration.')) {
    const segments = path.split('.');
    const field = segments[1];
    if (!['index', 'indices', 'item'].includes(field)) {
      return `'iteration' has no field '${field}'. Available: index, indices, item`;
    }
    if (field === 'item' && segments.length > 2 && context.insideSplitScope) {
      return validateSchemaFieldPath(
        segments.slice(2),
        context.splitItemSchemaFields,
        'the nearest Split iteration schema'
      );
    }
  }

  return null;
}

function validateStepReferencePath(
  path: string,
  previousSteps: StepInfo[],
  currentStepId?: string
): string | null {
  const parsed = parseStepReference(path);
  if (!parsed) {
    return null;
  }
  // `steps.__error` is the onError scope's pseudo-step carrying the failure
  // payload — always legal inside error handlers.
  if (parsed.stepId === '__error') {
    return null;
  }
  // Self-references are a save-time WARNING server-side, not an error.
  if (currentStepId && parsed.stepId === currentStepId) {
    return null;
  }
  const step = previousSteps.find((s) => s.id === parsed.stepId);
  if (!step) {
    return `Step '${parsed.stepId}' is not an upstream step here`;
  }
  if (!step.stepType) {
    return null;
  }
  const shape = getStepOutputShape(step.stepType);
  if (!shape) {
    return null;
  }

  const segments = parsed.rest.split('.');
  // Only the outputs value has a statically-declared shape; sibling fields
  // and bracket forms are never flagged (mirrors the save-time preflight).
  if (segments[0] !== 'outputs' || parsed.rest.includes('[')) {
    return null;
  }
  const after = segments[1];
  if (after === undefined || after === '') {
    return null;
  }

  const kind = shape.outputs?.kind;
  if (kind === 'array') {
    if (!/^-?\d+$/.test(after)) {
      return `'${step.name}' outputs an array — address elements by index (e.g. outputs.0), not '.${after}'`;
    }
    return null;
  }
  if (kind === 'object') {
    const fields = shape.outputs?.fields ?? [];
    if (fields.length > 0 && !fields.some((f) => f.name === after)) {
      const available = fields.map((f) => f.name).join(', ');
      return `'${step.name}' has no output field '${after}'. Available: ${available}`;
    }
  }
  return null;
}

function validateSchemaFieldPath(
  segments: string[],
  fields: SchemaField[] | undefined,
  scopeLabel: string
): string | null {
  // An empty/undeclared schema is unchecked — the runtime accepts any shape.
  if (!fields || fields.length === 0) {
    return null;
  }
  const [head, ...rest] = segments;
  const field = fields.find((f) => f.name === head);
  if (!field) {
    const available = fields.map((f) => f.name).join(', ');
    return `'${head}' is not declared in ${scopeLabel}. Available: ${available}`;
  }
  if (rest.length === 0) {
    return null;
  }
  // Only descend where nested properties are declared; other shapes are not
  // statically checkable.
  if (field.properties && field.properties.length > 0) {
    return validateSchemaFieldPath(rest, field.properties, scopeLabel);
  }
  return null;
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
