import type {
  ReportEditorConfig,
  ReportTableColumn,
  ReportTableInteractionButtonConfig,
  ReportWorkflowActionConfig,
} from '../../types';
import {
  formatCellValue,
  getReportRowValue,
  humanizeFieldName,
  renderDisplayTemplate,
  truncateCellText,
} from '../../utils';

export type TableColumn = {
  key: string;
  label?: string | null;
  displayField?: string | null;
  displayTemplate?: string | null;
  format?: string | null;
  type?: 'value' | 'chart' | 'workflow_button' | 'interaction_buttons' | null;
  chart?: ReportTableColumn['chart'];
  secondaryField?: string | null;
  linkField?: string | null;
  tooltipField?: string | null;
  pillVariants?: ReportTableColumn['pillVariants'];
  levels?: string[] | null;
  align?: 'left' | 'right' | 'center' | string | null;
  maxChars?: number | null;
  editable?: boolean | null;
  editor?: ReportEditorConfig | null;
  workflowAction?: ReportWorkflowActionConfig | null;
  interactionButtons?: ReportTableInteractionButtonConfig[];
};

/**
 * Per-column sizing spec, in px.
 *
 * `idealPx` is the width the column wants when the container has room —
 * sized to the longest *displayed* value on the current page (post
 * displayField/displayTemplate resolution, post format, post maxChars
 * truncation), floored so its own header never truncates.
 *
 * `minPx` is the narrowest the fit pass may compress it to; the cell's
 * truncate-with-tooltip rendering absorbs the difference. Rigid columns
 * (numbers, dates, pills, avatars, charts, action buttons) set
 * `minPx === idealPx` — truncating a right-aligned number is worse than
 * scrolling.
 */
export type ColumnSpec = {
  idealPx: number;
  minPx: number;
  flexible: boolean;
};

// Average glyph advance at cell font (text-sm / 14px Inter). Character-count
// heuristics only need to be close, and should err high: overshoot is
// corrected by the fit pass, undershoot ellipsizes values a couple of
// characters early (Inter digits run ~8.4px, so digit-heavy codes dominate).
const CH_PX = 8;
// Header glyphs render uppercase text-xs tracking-wide semibold — wide
// capitals (M, O, C, K) push the average well past the lowercase body text.
const HEADER_GLYPH_PX = 8.5;
// Header px-3 padding (24) + sort caret and gap (20) + a few px of slack so
// a column sitting exactly at its header floor doesn't ellipsize its label.
const HEADER_EXTRA_PX = 48;
// Cell px-3 padding (24) + rounding slack.
const CELL_PADDING_PX = 28;

// Width bounds for flexible text columns. The lower bound keeps short columns
// from collapsing; the upper bound keeps a single long column (descriptions,
// AI rationales) from monopolizing the table. Exported for tests.
export const MIN_TEXT_PX = Math.round(9 * CH_PX) + CELL_PADDING_PX;
export const MAX_TEXT_PX = Math.round(30 * CH_PX) + CELL_PADDING_PX;

const ACTION_COLUMN_PX = 160;
const CHART_COLUMN_PX = 160;

// The leading checkbox column when the table is selectable (w-10 / 2.5rem).
export const CHECKBOX_COLUMN_PX = 40;

const SAMPLE_LIMIT = 100;

// Fallback widths for formatted columns when there are no rows to measure.
const FORMAT_FALLBACK_WIDTHS: Record<string, number> = {
  date: 116,
  datetime: 184,
  bytes: 110,
  percent: 96,
  currency: 128,
  currency_compact: 110,
  number: 104,
  number_compact: 96,
  decimal: 110,
  bar_indicator: 140,
};

// Formats whose cells must never truncate (right-aligned numerics, dates).
const RIGID_VALUE_FORMATS = new Set([
  'date',
  'datetime',
  'bytes',
  'percent',
  'currency',
  'currency_compact',
  'number',
  'number_compact',
  'decimal',
]);

export function hasPositiveMaxChars(
  maxChars: number | null | undefined
): maxChars is number {
  return (
    typeof maxChars === 'number' && Number.isFinite(maxChars) && maxChars > 0
  );
}

export function isWorkflowButtonColumn(column: TableColumn): boolean {
  return (
    column.type === 'workflow_button' || column.workflowAction !== undefined
  );
}

export function isInteractionButtonsColumn(column: TableColumn): boolean {
  return (
    column.type === 'interaction_buttons' ||
    (column.interactionButtons?.length ?? 0) > 0
  );
}

export function isActionColumn(column: TableColumn): boolean {
  return isWorkflowButtonColumn(column) || isInteractionButtonsColumn(column);
}

export function defaultAlign(format?: string | null): TableColumn['align'] {
  if (!format) return undefined;
  const formatName = format.split(':', 1)[0];
  if (
    formatName === 'currency' ||
    formatName === 'currency_compact' ||
    formatName === 'number' ||
    formatName === 'number_compact' ||
    formatName === 'decimal' ||
    formatName === 'percent' ||
    formatName === 'bytes'
  ) {
    return 'right';
  }
  return undefined;
}

export function displayNameFromValue(raw: string): string {
  if (!raw) return '—';
  const local = raw.includes('@') ? raw.split('@')[0] : raw;
  return local
    .split(/[._-]+/)
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(' ');
}

export function getCellDisplayValue(
  row: Record<string, unknown>,
  column: TableColumn,
  value: unknown
) {
  if (column.displayTemplate) {
    const displayValue = renderDisplayTemplate(row, column.displayTemplate);
    if (displayValue.trim().length > 0) return displayValue;
  }
  if (column.displayField) {
    const displayValue = getReportRowValue(row, column.displayField);
    if (displayValue === null || displayValue === undefined) return value;
    if (typeof displayValue === 'string' && displayValue.trim().length === 0) {
      return value;
    }
    return displayValue;
  }
  return value;
}

function headerFloorPx(column: TableColumn): number {
  const label = column.label ?? humanizeFieldName(column.key);
  return Math.ceil(Array.from(label).length * HEADER_GLYPH_PX) + HEADER_EXTRA_PX;
}

function textWidthPx(chars: number): number {
  return Math.round(chars * CH_PX) + CELL_PADDING_PX;
}

function rigid(px: number): ColumnSpec {
  const width = Math.round(px);
  return { idealPx: width, minPx: width, flexible: false };
}

function clampPx(value: number, min: number, max: number): number {
  return Math.round(Math.min(Math.max(value, min), max));
}

/**
 * The string a cell will actually render, mirroring TableCellValue's display
 * pipeline. Sizing must measure this — not the raw value — or a 36-char UUID
 * behind a `displayField` showing "Acme" gets a 30ch column.
 */
function sampleDisplayTexts(
  column: TableColumn,
  rowObjects: Array<Record<string, unknown>>,
  formatName: string
): string[] {
  const count = Math.min(rowObjects.length, SAMPLE_LIMIT);
  const result: string[] = [];
  for (let i = 0; i < count; i += 1) {
    const row = rowObjects[i];
    const value = row[column.key];
    const display = getCellDisplayValue(row, column, value);

    if (formatName === 'pill' || formatName === 'bar_indicator') {
      const key = typeof display === 'string' ? display : String(display ?? '');
      result.push(key ? humanizeFieldName(key) : '');
      continue;
    }
    if (formatName === 'avatar_label') {
      const raw = typeof display === 'string' ? display : String(display ?? '');
      const name = raw ? displayNameFromValue(raw) : '';
      result.push(truncateCellText(name, column.maxChars).text);
      continue;
    }

    const text = formatCellValue(display, column.format ?? undefined);
    result.push(truncateCellText(text, column.maxChars).text);
  }
  return result;
}

function maxGlyphLength(samples: string[]): number {
  return samples.reduce(
    (acc, text) => Math.max(acc, Array.from(text).length),
    0
  );
}

/**
 * Compute the two-tier sizing spec for every column. Widths are ideals, not
 * final: `fitColumnWidths` reconciles them against the container.
 *
 * Every column resolves to a concrete width — no column is left auto/flex.
 * With `table-layout: fixed` + the table primitive's `min-w-max`, an auto
 * column with nowrap content expands to its full intrinsic width, and several
 * such columns blow the table up to thousands of px (the "only one column
 * visible" regression). The trailing filler col (no content, so it
 * contributes 0 to max-content) absorbs slack when the ideals underfill the
 * container.
 */
export function inferColumnSpecs(
  columns: TableColumn[],
  rowObjects: Array<Record<string, unknown>>
): ColumnSpec[] {
  return columns.map((column) => inferColumnSpec(column, rowObjects));
}

function inferColumnSpec(
  column: TableColumn,
  rowObjects: Array<Record<string, unknown>>
): ColumnSpec {
  if (isActionColumn(column)) {
    return rigid(ACTION_COLUMN_PX);
  }
  if (column.type === 'chart') {
    return rigid(CHART_COLUMN_PX);
  }

  const header = headerFloorPx(column);
  const formatName = (column.format ?? '').split(':', 1)[0];
  const samples = sampleDisplayTexts(column, rowObjects, formatName);
  const maxLen = maxGlyphLength(samples);

  // Pills and avatars render decorated content; their widths come from the
  // decorated sample (humanized label / derived display name). Pills are
  // rigid (a Badge doesn't truncate); avatar and bar-indicator cells render
  // their labels through truncate-with-tooltip, so they can lend width under
  // compression like any text column.
  if (formatName === 'pill') {
    const pillPx = clampPx(maxLen * 6.5 + 58, 96, 200);
    return rigid(Math.max(pillPx, header));
  }
  if (formatName === 'avatar_label') {
    const ideal = Math.max(clampPx(maxLen * 7 + 64, 140, 240), header);
    const min = Math.min(Math.max(140, header), ideal);
    return { idealPx: ideal, minPx: min, flexible: true };
  }
  if (formatName === 'bar_indicator') {
    const measured =
      maxLen > 0
        ? textWidthPx(maxLen) + 32
        : FORMAT_FALLBACK_WIDTHS.bar_indicator;
    const ideal = Math.max(measured, header);
    // Short level labels ("Low", "Medium") keep their width — compressing
    // them buys a handful of px at a large legibility cost. Only labels
    // beyond ~120px lend width under compression.
    const min = Math.min(Math.max(120, header), ideal);
    return { idealPx: ideal, minPx: min, flexible: true };
  }

  if (RIGID_VALUE_FORMATS.has(formatName) && !hasPositiveMaxChars(column.maxChars)) {
    const measured =
      maxLen > 0 ? textWidthPx(maxLen) : FORMAT_FALLBACK_WIDTHS[formatName];
    return rigid(Math.max(measured, header));
  }

  // Flexible text column. maxChars is a cap on the ideal (and the truncation
  // cutoff applied during sampling) — not a width promise: short data yields
  // a narrow column even when the author allows 50 chars.
  const min = Math.max(header, MIN_TEXT_PX);
  const cap = hasPositiveMaxChars(column.maxChars)
    ? // +3 so the appended "..." from truncateCellText still fits.
      textWidthPx(Math.trunc(column.maxChars) + 3)
    : MAX_TEXT_PX;
  const dataIdeal = maxLen > 0 ? textWidthPx(maxLen) : 0;
  const ideal = Math.max(min, Math.min(dataIdeal, cap));
  return {
    idealPx: Math.round(ideal),
    minPx: Math.round(min),
    flexible: true,
  };
}

// Under compression, a flexible column may dip this far below its minimum to
// absorb a sliver of residual deficit. The min floors carry built-in slack
// (padding and caret allowances round up), so a few px cost nothing visible —
// while without this, a mostly-rigid table can end up with a 3–10px
// scrollbar that scrolls nothing useful.
const GRACE_PX = 6;

/**
 * Reconcile ideal column widths against the container. When the ideals fit,
 * every column gets its ideal (the filler col absorbs the slack). When they
 * don't, flexible columns shrink in proportion to their headroom
 * (`ideal - min`); proportional-to-headroom cuts can never push a column
 * below its min in a single pass. A residual sliver (≤ GRACE_PX per flexible
 * column) is shaved below the mins; beyond that, the table legitimately
 * scrolls.
 *
 * `availablePx === null` means the container width is unknown (first paint,
 * jsdom) — return the ideals unchanged.
 */
export function fitColumnWidths(
  specs: ColumnSpec[],
  availablePx: number | null
): number[] {
  const ideals = specs.map((spec) => spec.idealPx);
  if (availablePx === null || availablePx <= 0) {
    return ideals;
  }

  const total = ideals.reduce((acc, width) => acc + width, 0);
  const deficit = total - availablePx;
  if (deficit <= 0) {
    return ideals;
  }

  const headrooms = specs.map((spec) =>
    spec.flexible ? Math.max(0, spec.idealPx - spec.minPx) : 0
  );
  const totalHeadroom = headrooms.reduce((acc, h) => acc + h, 0);
  if (totalHeadroom <= 0 && deficit > countFlexible(specs) * GRACE_PX) {
    return ideals;
  }

  const shrink = Math.min(deficit, totalHeadroom);
  const widths = specs.map((spec, index) => {
    if (headrooms[index] <= 0) return spec.idealPx;
    const cut = (shrink * headrooms[index]) / totalHeadroom;
    return Math.max(spec.minPx, Math.floor(spec.idealPx - cut));
  });

  // Grace pass: if only a sliver of deficit remains once every flexible
  // column sits at its min, spread it below the mins (bounded per column)
  // instead of rendering a near-useless horizontal scrollbar.
  let residual = widths.reduce((acc, w) => acc + w, 0) - availablePx;
  const flexibleCount = countFlexible(specs);
  if (
    residual > 0 &&
    flexibleCount > 0 &&
    residual <= flexibleCount * GRACE_PX
  ) {
    for (let i = 0; i < widths.length && residual > 0; i += 1) {
      if (!specs[i].flexible) continue;
      const dip = Math.min(GRACE_PX, residual);
      widths[i] -= dip;
      residual -= dip;
    }
  }
  return widths;
}

function countFlexible(specs: ColumnSpec[]): number {
  return specs.reduce((acc, spec) => acc + (spec.flexible ? 1 : 0), 0);
}
