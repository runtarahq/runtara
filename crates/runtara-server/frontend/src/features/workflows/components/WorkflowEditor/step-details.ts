/**
 * What a step is configured to do, as label/value rows.
 *
 * The canvas shows these on the card (detailed mode) and in the hover preview,
 * so a step's inputs are readable without opening its editor. Rows come from
 * the editor's node data — `inputMapping` in its UI shape, which the loader
 * normalizes every step type into — plus a few step-specific fields.
 *
 * Keep this pure: the canvas layout calls it to size cards before render.
 */

export type StepDetailKind =
  'reference' | 'template' | 'composite' | 'literal' | 'list';

export interface StepDetailRow {
  key: string;
  label: string;
  value: string;
  kind: StepDetailKind;
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

function labelFor(key: string, stepType?: string): string {
  if (key === 'value' && stepType && ITEMS_VALUE_STEP_TYPES.has(stepType)) {
    return 'items';
  }
  if (isDurationKey(key)) return key.slice(0, -2);
  return key;
}

function describeComposite(value: unknown): string {
  if (Array.isArray(value)) {
    return `[${value.length} item${value.length === 1 ? '' : 's'}]`;
  }
  if (value && typeof value === 'object') {
    const keys = Object.keys(value);
    const shown = keys.slice(0, 4).join(', ');
    return `{ ${shown}${keys.length > 4 ? ', …' : ''} }`;
  }
  return String(value);
}

function formatEntry(entry: MappingEntry, key: string): StepDetailRow['value'] {
  const { value, valueType } = entry;

  if (valueType === 'reference' || valueType === 'template') {
    return truncate(String(value));
  }
  if (valueType === 'composite') {
    return describeComposite(value);
  }
  if (typeof value === 'number' && isDurationKey(key)) {
    return formatDuration(value);
  }
  if (typeof value === 'string' && isDurationKey(key) && /^\d+$/.test(value)) {
    return formatDuration(Number(value));
  }
  if (Array.isArray(value) || (value && typeof value === 'object')) {
    return truncate(JSON.stringify(value));
  }
  return truncate(String(value));
}

function kindFor(entry: MappingEntry): StepDetailKind {
  if (entry.valueType === 'reference') return 'reference';
  if (entry.valueType === 'template') return 'template';
  if (entry.valueType === 'composite') return 'composite';
  return 'literal';
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

/** Every configured input of a step, in mapping order. */
export function stepDetailRows(data: StepDetailData): StepDetailRow[] {
  if (data.stepType && NO_DETAIL_STEP_TYPES.has(data.stepType)) return [];

  const entries = mappingEntries(data);
  const rows: StepDetailRow[] = [];

  for (const entry of entries) {
    const key = entry.type;
    if (!key || HIDDEN_MAPPING_KEYS.has(key) || isConditionKey(key)) continue;
    if (isEmptyValue(entry.value)) continue;

    rows.push({
      key,
      label: labelFor(key, data.stepType),
      value: formatEntry(entry, key),
      kind: kindFor(entry),
    });
  }

  if (data.stepType === 'WaitForSignal') {
    const fields = responseFieldNames(entries);
    if (fields.length > 0) {
      rows.unshift({
        key: 'responseSchema',
        label: 'asks for',
        value: truncate(fields.join(', ')),
        kind: 'list',
      });
    }
  }

  if (
    data.stepType === 'Split' &&
    typeof data.splitParallelism === 'number' &&
    data.splitParallelism > 0
  ) {
    rows.push({
      key: 'parallelism',
      label: 'parallelism',
      value: String(data.splitParallelism),
      kind: 'literal',
    });
  }

  if (data.stepType === 'EmbedWorkflow' && !isEmptyValue(data.childVersion)) {
    rows.push({
      key: 'childVersion',
      label: 'version',
      value: String(data.childVersion),
      kind: 'literal',
    });
  }

  return rows;
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
