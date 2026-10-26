/**
 * What a step is configured to do, as rows a person can read.
 *
 * The canvas shows these on the card (detailed mode) and in the hover preview,
 * so a step's inputs are readable without opening its editor. Rows come from
 * the editor's node data — `inputMapping` in its UI shape, which the loader
 * normalizes every step type into — plus a few step-specific fields.
 *
 * Rows say where a value comes from rather than echoing the DSL: `data.id`
 * reads "Input · id", `steps.read.outputs.rows` reads "Read tenant · rows",
 * and an object reads as the fields it sets. Labels are the capability's own
 * display names when the caller passes them.
 *
 * Keep this pure: the canvas layout calls it (without a context) to size cards
 * before render, so the number of rows must never depend on the context.
 */

import {
  isJsonTextHint,
  parseJsonText,
} from '@/features/workflows/utils/mapping-serialization';

export type StepDetailKind =
  'reference' | 'template' | 'composite' | 'literal' | 'list';

/** Where a referenced value comes from, split for display. */
export interface StepDetailSource {
  /** "Input", "Item", "Variable", "Run", "Error", or the source step's name. */
  source: string;
  /** Field path within the source; empty when the whole source is used. */
  path: string;
}

export interface StepDetailRow {
  key: string;
  label: string;
  /** Plain-text rendering of the value (titles, truncation, tests). */
  value: string;
  kind: StepDetailKind;
  /** Set for references: the value's source and path. */
  reference?: StepDetailSource;
  /** Set for composite objects: one row per field they set. */
  children?: StepDetailRow[];
  required?: boolean;
}

/** Optional knowledge that makes rows friendlier. Never changes row count. */
export interface StepDetailContext {
  /** The capability's declared inputs, in declaration order. */
  inputFields?: Array<{
    name: string;
    displayName?: string | null;
    required?: boolean;
  }>;
  /** Step id → step name, to name the source of `steps.<id>` references. */
  stepNames?: ReadonlyMap<string, string>;
  /** `item` inside a Split, where `data.*` is the current item. */
  dataScope?: 'input' | 'item';
}

interface MappingEntry {
  type?: string;
  value?: unknown;
  valueType?: string;
  typeHint?: string;
}

interface StepDetailData {
  stepType?: string;
  inputMapping?: unknown;
  childVersion?: unknown;
  splitParallelism?: unknown;
}

/** Rows a detailed card shows before collapsing the rest into "+N more". */
export const MAX_CARD_DETAIL_ROWS = 3;

/**
 * Mapping entries that are configuration plumbing rather than inputs a reader
 * cares about, or that another part of the card already shows (Switch cases
 * are the case handles, AiAgent tools and memory are listed on its card).
 */
const HIDDEN_MAPPING_KEYS = new Set([
  'cases',
  'routingMode',
  'tools',
  'mcpToolsets',
  'memoryEnabled',
  'memoryProviderStepId',
  'memoryConversationId',
  'memoryMaxMessages',
  'memoryStrategy',
  'responseSchema',
]);

/** Step types whose `value` mapping is the collection they iterate over. */
const ITEMS_VALUE_STEP_TYPES = new Set(['Split', 'Filter', 'GroupBy']);

/** Step types whose cards carry no detail rows of their own. */
const NO_DETAIL_STEP_TYPES = new Set([
  // The condition is the card's subtitle; its hover preview has it in full.
  'Conditional',
  // The AiAgent card lists its tools and memory itself.
  'AiAgent',
]);

const MAX_VALUE_LENGTH = 160;

/** Words shown in capitals when a key is turned into a label. */
const ACRONYMS = new Set([
  'id',
  'url',
  'uri',
  'sql',
  'api',
  'http',
  'json',
  'xml',
  'csv',
  'html',
  'ip',
  'llm',
  'mcp',
  'sftp',
  's3',
]);

/** `instance_id` → "Instance ID", `schemaName` → "Schema name". */
export function humanizeKey(key: string): string {
  const words = key
    .replace(/([a-z0-9])([A-Z])/g, '$1 $2')
    .split(/[\s_\-.]+/)
    .filter(Boolean)
    .map((word) => word.toLowerCase());
  if (words.length === 0) return key;
  return words
    .map((word, index) => {
      if (word === 'ids') return 'IDs';
      if (ACRONYMS.has(word)) return word.toUpperCase();
      return index === 0 ? word.charAt(0).toUpperCase() + word.slice(1) : word;
    })
    .join(' ');
}

function isConditionKey(key: string): boolean {
  return key.startsWith('condition.') || key.startsWith('expression.');
}

function isEmptyValue(value: unknown): boolean {
  if (value === undefined || value === null || value === '') return true;
  if (Array.isArray(value)) return value.length === 0;
  if (typeof value === 'object') return Object.keys(value).length === 0;
  return false;
}

function truncate(text: string): string {
  return text.length > MAX_VALUE_LENGTH
    ? `${text.slice(0, MAX_VALUE_LENGTH - 1)}…`
    : text;
}

function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  const seconds = ms / 1000;
  if (seconds < 60) return `${Number(seconds.toFixed(2))} s`;
  const minutes = seconds / 60;
  if (minutes < 60) return `${Number(minutes.toFixed(1))} min`;
  return `${Number((minutes / 60).toFixed(1))} h`;
}

function isDurationKey(key: string): boolean {
  return /Ms$/.test(key);
}

function labelFor(
  key: string,
  stepType: string | undefined,
  context: StepDetailContext | undefined
): string {
  const declared = context?.inputFields?.find((field) => field.name === key);
  if (declared?.displayName) return declared.displayName;
  if (key === 'value' && stepType && ITEMS_VALUE_STEP_TYPES.has(stepType)) {
    return 'Items';
  }
  if (isDurationKey(key)) return humanizeKey(key.slice(0, -2));
  return humanizeKey(key);
}

/** Split a reference path into what it reads from and the field within it. */
export function describeReference(
  raw: string,
  context?: StepDetailContext
): StepDetailSource {
  const path = raw.trim();
  const startsWith = (prefix: string) =>
    path === prefix ||
    path.startsWith(`${prefix}.`) ||
    path.startsWith(`${prefix}[`);
  const rest = (prefix: string) => path.slice(prefix.length).replace(/^\./, '');

  const dataSource = context?.dataScope === 'item' ? 'Item' : 'Input';
  if (startsWith('workflow.inputs.data')) {
    return { source: dataSource, path: rest('workflow.inputs.data') };
  }
  if (startsWith('data')) {
    return { source: dataSource, path: rest('data') };
  }

  const variablePrefix = startsWith('workflow.inputs.variables')
    ? 'workflow.inputs.variables'
    : startsWith('variables')
      ? 'variables'
      : null;
  if (variablePrefix) {
    const name = rest(variablePrefix);
    // Built-ins (_instance_id, _workflow_id, _tenant_id) describe the run.
    if (name.startsWith('_')) {
      return { source: 'Run', path: humanizeKey(name.slice(1)) };
    }
    return { source: 'Variable', path: name };
  }

  if (startsWith('steps')) {
    const [stepId, ...fields] = rest('steps').split('.');
    if (stepId === '__error') {
      return { source: 'Error', path: fields.join('.') };
    }
    const fieldPath = fields[0] === 'outputs' ? fields.slice(1) : fields;
    return {
      source: context?.stepNames?.get(stepId) || stepId,
      path: fieldPath.join('.'),
    };
  }

  return { source: '', path };
}

export function referenceText(reference: StepDetailSource): string {
  if (!reference.source) return reference.path;
  return reference.path
    ? `${reference.source} · ${reference.path}`
    : reference.source;
}

/** `Hello {{ data.name }}` → `Hello ‹Input · name›`; other expressions stay. */
function formatTemplate(template: string, context?: StepDetailContext): string {
  return template.replace(
    /\{\{\s*([A-Za-z_][\w.[\]]*)\s*\}\}/g,
    (_, expression: string) =>
      `‹${referenceText(describeReference(expression, context))}›`
  );
}

function formatLiteral(value: unknown, key: string): string {
  if (typeof value === 'boolean') return value ? 'Yes' : 'No';
  if (typeof value === 'number') {
    return isDurationKey(key) ? formatDuration(value) : value.toLocaleString();
  }
  if (typeof value === 'string' && isDurationKey(key) && /^\d+$/.test(value)) {
    return formatDuration(Number(value));
  }
  if (Array.isArray(value)) {
    return `${value.length} item${value.length === 1 ? '' : 's'}`;
  }
  if (value && typeof value === 'object') {
    // A card never shows raw JSON: an object reads as the fields it holds,
    // like a built (composite) value does.
    const keys = Object.keys(value as Record<string, unknown>);
    return truncate(
      keys.length > 0 ? keys.map(humanizeKey).join(', ') : 'Empty object'
    );
  }
  return truncate(String(value));
}

function isMappingEntry(value: unknown): value is MappingEntry {
  return (
    !!value &&
    typeof value === 'object' &&
    !Array.isArray(value) &&
    'valueType' in value
  );
}

function buildRow(
  key: string,
  label: string,
  entry: MappingEntry,
  context: StepDetailContext | undefined
): StepDetailRow {
  const { value, valueType } = entry;

  if (valueType === 'reference') {
    const reference = describeReference(String(value), context);
    return {
      key,
      label,
      value: truncate(referenceText(reference)),
      kind: 'reference',
      reference,
    };
  }

  if (valueType === 'template') {
    return {
      key,
      label,
      value: truncate(formatTemplate(String(value), context)),
      kind: 'template',
    };
  }

  if (valueType === 'composite') {
    if (Array.isArray(value)) {
      return {
        key,
        label,
        value: `${value.length} item${value.length === 1 ? '' : 's'}`,
        kind: 'composite',
      };
    }
    const fields: Record<string, unknown> =
      value && typeof value === 'object'
        ? (value as Record<string, unknown>)
        : {};
    const children = Object.entries(fields)
      .filter(
        (pair): pair is [string, MappingEntry] =>
          isMappingEntry(pair[1]) && !isEmptyValue(pair[1].value)
      )
      .map(([childKey, child]) =>
        buildRow(`${key}.${childKey}`, humanizeKey(childKey), child, context)
      );
    return {
      key,
      label,
      // An object reads as the fields it sets; their values are in children.
      value: truncate(
        children.length > 0
          ? children.map((child) => child.label).join(', ')
          : `${Object.keys(fields).length} fields`
      ),
      kind: 'composite',
      children,
    };
  }

  const literal = literalValue(entry);

  // A condition held as a plain value (object-model query `condition`, a
  // Filter-style predicate) reads as a sentence, as on Conditional cards.
  if (isConditionExpression(literal)) {
    const sentence = describeCondition(literal, context);
    if (sentence) {
      return { key, label, value: truncate(sentence), kind: 'literal' };
    }
  }

  return { key, label, value: formatLiteral(literal, key), kind: 'literal' };
}

/**
 * The value an immediate holds. Text under a JSON hint (a literal being
 * edited in the step panel) reads as what a save writes for it, by the save
 * path's own rule, not as raw JSON.
 */
function literalValue({ value, typeHint }: MappingEntry): unknown {
  return typeof value === 'string' && isJsonTextHint(typeHint)
    ? parseJsonText(value, typeHint)
    : value;
}

function mappingEntries(data: StepDetailData): MappingEntry[] {
  return Array.isArray(data.inputMapping)
    ? (data.inputMapping as MappingEntry[]).filter(
        (entry) => entry && typeof entry === 'object'
      )
    : [];
}

function responseFieldNames(entries: MappingEntry[]): string[] {
  const schema = entries.find((entry) => entry.type === 'responseSchema');
  if (!Array.isArray(schema?.value)) return [];
  return (schema.value as Array<{ name?: unknown }>)
    .map((field) => (typeof field?.name === 'string' ? field.name : ''))
    .filter(Boolean);
}

/**
 * Required inputs first, then the capability's declaration order, then
 * anything it does not declare. Stable, so steps without a capability keep
 * their mapping order.
 */
function orderRows(
  rows: StepDetailRow[],
  context: StepDetailContext | undefined
): StepDetailRow[] {
  const fields = context?.inputFields;
  if (!fields || fields.length === 0) return rows;
  const rank = (row: StepDetailRow) => {
    const index = fields.findIndex((field) => field.name === row.key);
    if (index === -1) return 2 * fields.length + 1;
    return (fields[index].required ? 0 : fields.length) + index;
  };
  return rows
    .map((row, index) => ({ row, index }))
    .sort((a, b) => rank(a.row) - rank(b.row) || a.index - b.index)
    .map(({ row }) => row);
}

/** Every configured input of a step, as readable rows. */
export function stepDetailRows(
  data: StepDetailData,
  context?: StepDetailContext
): StepDetailRow[] {
  if (data.stepType && NO_DETAIL_STEP_TYPES.has(data.stepType)) return [];

  const entries = mappingEntries(data);
  const rows: StepDetailRow[] = [];

  for (const entry of entries) {
    const key = entry.type;
    if (!key || HIDDEN_MAPPING_KEYS.has(key) || isConditionKey(key)) continue;
    if (isEmptyValue(entry.value)) continue;

    const row = buildRow(
      key,
      labelFor(key, data.stepType, context),
      entry,
      context
    );
    const declared = context?.inputFields?.find((field) => field.name === key);
    if (declared?.required) row.required = true;
    rows.push(row);
  }

  const ordered = orderRows(rows, context);

  if (data.stepType === 'WaitForSignal') {
    const fields = responseFieldNames(entries);
    if (fields.length > 0) {
      ordered.unshift({
        key: 'responseSchema',
        label: 'Asks for',
        value: truncate(fields.map(humanizeKey).join(', ')),
        kind: 'list',
      });
    }
  }

  if (
    data.stepType === 'Split' &&
    typeof data.splitParallelism === 'number' &&
    data.splitParallelism > 0
  ) {
    ordered.push({
      key: 'parallelism',
      label: 'Parallelism',
      value: `${data.splitParallelism} at a time`,
      kind: 'literal',
    });
  }

  if (data.stepType === 'EmbedWorkflow' && !isEmptyValue(data.childVersion)) {
    ordered.push({
      key: 'childVersion',
      label: 'Version',
      value: String(data.childVersion),
      kind: 'literal',
    });
  }

  return ordered;
}

interface ConditionLike {
  op?: string;
  arguments?: unknown[];
}

const BINARY_OPERATORS: Record<string, string> = {
  EQ: 'is',
  NE: 'is not',
  GT: '>',
  GTE: '≥',
  LT: '<',
  LTE: '≤',
  IN: 'is one of',
  NOT_IN: 'is not one of',
  CONTAINS: 'contains',
  STARTS_WITH: 'starts with',
  ENDS_WITH: 'ends with',
};

const UNARY_SUFFIX: Record<string, string> = {
  IS_EMPTY: 'is empty',
  IS_NOT_EMPTY: 'is not empty',
  IS_DEFINED: 'is set',
};

function isCondition(value: unknown): value is ConditionLike {
  return !!value && typeof value === 'object' && 'op' in value;
}

/**
 * Whether a value is a condition operation, as the DSL reads one: an `op`
 * and its `arguments`, with no `type` other than `"operation"`. A payload
 * that merely has an `op` key, like a JSON Patch operation, is not one.
 */
export function isConditionExpression(value: unknown): value is ConditionLike {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    return false;
  }
  const { op, arguments: args, type } = value as Record<string, unknown>;
  return (
    typeof op === 'string' &&
    Array.isArray(args) &&
    (type === undefined || type === 'operation')
  );
}

function describeOperand(
  operand: unknown,
  context: StepDetailContext | undefined
): string {
  if (isCondition(operand)) {
    return `(${describeCondition(operand, context)})`;
  }
  if (operand && typeof operand === 'object' && 'value' in operand) {
    const { value, valueType } = operand as MappingEntry;
    if (valueType === 'reference') {
      return referenceText(describeReference(String(value), context));
    }
    if (typeof value === 'string') return `"${value}"`;
    if (Array.isArray(value))
      return value.map((item) => String(item)).join(', ');
    return String(value);
  }
  return typeof operand === 'string' ? operand : String(operand ?? '');
}

/**
 * A condition as a sentence: `Input · stage is "intake"`, `Order total ≥ 100
 * and Input · express is true`. References name their source like detail rows.
 */
export function describeCondition(
  condition: unknown,
  context?: StepDetailContext
): string {
  if (!isCondition(condition) || !condition.op) return '';
  const op = condition.op.toUpperCase();
  const args = Array.isArray(condition.arguments) ? condition.arguments : [];
  const operand = (index: number) => describeOperand(args[index], context);

  if (op === 'AND' || op === 'OR') {
    return args
      .map((_, index) => operand(index))
      .join(op === 'AND' ? ' and ' : ' or ');
  }
  if (op === 'NOT') return `not ${operand(0)}`;
  if (op === 'LENGTH') return `length of ${operand(0)}`;
  if (UNARY_SUFFIX[op]) return `${operand(0)} ${UNARY_SUFFIX[op]}`;
  if (BINARY_OPERATORS[op]) {
    return `${operand(0)} ${BINARY_OPERATORS[op]} ${operand(1)}`;
  }
  return `${humanizeKey(op).toLowerCase()}(${args
    .map((_, index) => operand(index))
    .join(', ')})`;
}

/**
 * Lines a detailed card renders: up to MAX_CARD_DETAIL_ROWS rows, or one row
 * fewer plus a "+N more" line when the step has more than fit.
 */
export function cardDetailLayout(rows: StepDetailRow[]): {
  visible: StepDetailRow[];
  hiddenCount: number;
  lineCount: number;
} {
  if (rows.length <= MAX_CARD_DETAIL_ROWS) {
    return { visible: rows, hiddenCount: 0, lineCount: rows.length };
  }
  const visible = rows.slice(0, MAX_CARD_DETAIL_ROWS - 1);
  return {
    visible,
    hiddenCount: rows.length - visible.length,
    lineCount: MAX_CARD_DETAIL_ROWS,
  };
}
