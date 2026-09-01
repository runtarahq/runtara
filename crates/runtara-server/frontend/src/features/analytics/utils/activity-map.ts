import type { MetricsBucket } from '@/generated/RuntaraRuntimeApi';
import type { DateRangeOption } from '@/shared/components/date-range-selector';

/**
 * Layout for the activity map at one period.
 *
 * Sixty columns at every period, so the map keeps a constant width and only its
 * resolution changes. Rows subdivide a column: at 24 hours a column is 24
 * minutes tall, made of four six-minute cells. `unitMinutes` must match the
 * bucket width `useTenantMetrics` requests for the same period, or the map
 * would label cells with a grain the data does not have.
 */
export interface ActivityMapConfig {
  cols: number;
  rows: number;
  unitMinutes: number;
  /** Human description of one cell, shown under the card title. */
  grain: string;
}

export const ACTIVITY_MAP_CONFIG: Record<DateRangeOption, ActivityMapConfig> = {
  '1h': { cols: 60, rows: 1, unitMinutes: 1, grain: 'minute' },
  '24h': { cols: 60, rows: 4, unitMinutes: 6, grain: '6 minutes' },
  '7d': { cols: 60, rows: 7, unitMinutes: 24, grain: '24 minutes' },
  '30d': { cols: 60, rows: 6, unitMinutes: 120, grain: '2 hours' },
  '90d': { cols: 60, rows: 6, unitMinutes: 360, grain: '6 hours' },
};

export interface ActivityCell {
  key: string;
  startMs: number;
  endMs: number;
  total: number;
  success: number;
  failed: number;
  cancelled: number;
  /** 0 for an empty cell, otherwise one of four steps up to 1. */
  intensity: number;
  /** Cell failure rate, 0-1. */
  failRate: number;
  /**
   * True only when this interval failed materially worse than the window did.
   *
   * Marking every cell that contains any failure sounds right and is useless in
   * practice: a busy window has some failures nearly everywhere, so the marker
   * fires on almost every cell, stops distinguishing anything, and destroys the
   * density read underneath it. This fires on intervals worth looking at.
   */
  elevated: boolean;
}

export interface ActivityRow {
  label: string;
  cells: ActivityCell[];
}

export interface ActivityXLabel {
  /** 1-based grid column this label starts at. */
  column: number;
  span: number;
  label: string;
  alignEnd: boolean;
}

export interface ActivityMap {
  rows: ActivityRow[];
  xLabels: ActivityXLabel[];
  cols: number;
  total: number;
  success: number;
  failed: number;
  cancelled: number;
  /** The busiest single cell, or null when the window holds no runs. */
  peak: ActivityCell | null;
  /** How many cells contain at least one failure, elevated or not. */
  cellsWithFailures: number;
  /** How many of those cleared the elevated threshold. */
  elevatedCells: number;
}

const EMPTY: ActivityMap = {
  rows: [],
  xLabels: [],
  cols: 0,
  total: 0,
  success: 0,
  failed: 0,
  cancelled: 0,
  peak: null,
  cellsWithFailures: 0,
  elevatedCells: 0,
};

/**
 * Four intensity steps rather than a continuous ramp.
 *
 * A continuous scale makes one busy cell wash out every other, which is the
 * failure mode of most heatmaps: the eye reads "empty" where the value is
 * merely small. Stepping keeps a single run visible against a peak of hundreds.
 */
/**
 * Whether a cell's failure rate is worth flagging.
 *
 * Twice the window's own rate, and at least two failures so a single bad run in
 * a quiet interval does not light up. Against a clean window - baseline at or
 * near zero - any repeated failure is by definition elevated, which is what the
 * ELEVATED_FLOOR fallback covers.
 */
const ELEVATED_MULTIPLE = 2;
const ELEVATED_MIN_FAILURES = 2;
const ELEVATED_FLOOR = 0.1;

function isElevated(cell: ActivityCell, baseline: number): boolean {
  if (cell.failed < ELEVATED_MIN_FAILURES) return false;
  const threshold = Math.max(baseline * ELEVATED_MULTIPLE, ELEVATED_FLOOR);
  return cell.failRate >= threshold;
}

/** The four fill levels a non-empty cell can take. */
const LEVELS = [0.22, 0.45, 0.7, 1] as const;

/**
 * Bin cell volumes by quartile rather than against the maximum.
 *
 * Scaling to the peak sounds more honest and reads far worse: when most
 * intervals sit between half and three-quarters of the busiest one - which is
 * what a steady workload looks like - every cell lands in the same band and the
 * map becomes a flat wash. Ranking spreads the cells across all four levels, so
 * the shape of the window is visible whatever the absolute numbers are.
 *
 * The cost is that a level means "busier than most" rather than "near the
 * peak", which is why the exact counts stay in the tooltip and the busiest
 * interval is named in text underneath.
 */
function buildScale(values: number[]): (value: number) => number {
  const sorted = values.filter((v) => v > 0).sort((a, b) => a - b);
  if (sorted.length === 0) return () => 0;

  const quantile = (fraction: number) =>
    sorted[Math.min(sorted.length - 1, Math.floor(fraction * sorted.length))];
  const cuts = [quantile(0.25), quantile(0.5), quantile(0.75)];

  // No spread at all - every busy cell is equally busy. Say so plainly rather
  // than inventing four bands out of a single value.
  if (cuts[0] === sorted[sorted.length - 1]) {
    return (value) => (value > 0 ? LEVELS[1] : 0);
  }

  const max = sorted[sorted.length - 1];
  return (value) => {
    if (value <= 0) return 0;
    // The busiest interval is always fully saturated: the legend says "Most",
    // and quartile cuts on a small sample can otherwise leave the peak a shade
    // short of the top.
    if (value >= max) return LEVELS[3];
    if (value <= cuts[0]) return LEVELS[0];
    if (value <= cuts[1]) return LEVELS[1];
    if (value <= cuts[2]) return LEVELS[2];
    return LEVELS[3];
  };
}

/**
 * Fold the metrics buckets into the map's grid.
 *
 * Every returned bucket gets a cell. The grid grows a column rather than
 * dropping the overflow: the spine is inclusive of both edges so it carries one
 * more entry than `cols * rows`, and discarding it meant the map's "busiest
 * interval" could disagree with the same figure computed over the full series.
 *
 * Cells are laid out column-major - a column is one coarse interval, its rows
 * are the finer slots inside it - so time runs left to right the way the axis
 * labels claim.
 */
export function buildActivityMap(
  buckets: MetricsBucket[] | undefined | null,
  config: ActivityMapConfig
): ActivityMap {
  if (!buckets || buckets.length === 0) return EMPTY;

  const windowed = buckets;
  if (windowed.length === 0) return EMPTY;

  const unitMs = config.unitMinutes * 60_000;
  const cells: ActivityCell[] = windowed.map((bucket, index) => {
    const startMs = bucket.bucket_time
      ? new Date(bucket.bucket_time).getTime()
      : index * unitMs;
    const failed = bucket.failure_count ?? 0;
    const cancelled = bucket.cancelled_count ?? 0;
    return {
      key: bucket.bucket_time ?? String(index),
      startMs,
      endMs: startMs + unitMs,
      total: bucket.invocation_count ?? 0,
      success: bucket.success_count ?? 0,
      failed,
      cancelled,
      intensity: 0,
      failRate: 0,
      elevated: false,
    };
  });

  const scale = buildScale(cells.map((c) => c.total));
  const windowRuns = cells.reduce((sum, c) => sum + c.total, 0);
  const windowFailed = cells.reduce((sum, c) => sum + c.failed, 0);
  const baseline = windowRuns > 0 ? windowFailed / windowRuns : 0;

  for (const cell of cells) {
    cell.intensity = scale(cell.total);
    cell.failRate = cell.total > 0 ? cell.failed / cell.total : 0;
    cell.elevated = isElevated(cell, baseline);
  }

  // Column-major: column c owns cells [c * rows, (c + 1) * rows).
  const filledCols = Math.ceil(cells.length / config.rows);
  const at = (row: number, col: number): ActivityCell | undefined =>
    cells[col * config.rows + row];

  // One unit for the whole column, chosen once. Deciding per row produced
  // "+96m, +2h, +144m" in the same stack, because only some offsets happened to
  // land on a whole hour.
  const useHours = Array.from(
    { length: config.rows },
    (_, r) => r * config.unitMinutes
  ).every((offset) => offset % 60 === 0);

  const rows: ActivityRow[] = [];
  for (let r = 0; r < config.rows; r++) {
    const offset = r * config.unitMinutes;
    const label =
      offset === 0 ? '+0' : useHours ? `+${offset / 60}h` : `+${offset}m`;
    const rowCells: ActivityCell[] = [];
    for (let c = 0; c < filledCols; c++) {
      const cell = at(r, c);
      if (cell) rowCells.push(cell);
    }
    rows.push({ label, cells: rowCells });
  }

  const total = cells.reduce((sum, c) => sum + c.total, 0);
  const peak = cells.reduce<ActivityCell | null>(
    (best, c) => (best === null || c.total > best.total ? c : best),
    null
  );

  return {
    rows,
    xLabels: buildXLabels(cells, config, filledCols),
    cols: filledCols,
    total,
    success: cells.reduce((sum, c) => sum + c.success, 0),
    failed: cells.reduce((sum, c) => sum + c.failed, 0),
    cancelled: cells.reduce((sum, c) => sum + c.cancelled, 0),
    peak: peak && peak.total > 0 ? peak : null,
    cellsWithFailures: cells.filter((c) => c.failed > 0).length,
    elevatedCells: cells.filter((c) => c.elevated).length,
  };
}

/**
 * Five evenly spaced axis labels that never overlap.
 *
 * Each label occupies a span of columns, so a naive five-way split can collide
 * near the right edge. Labels that would overlap the previous one are dropped,
 * except the last - which is kept and pushes earlier ones out instead, because
 * "where does this window end" is the label people look for.
 */
function buildXLabels(
  cells: ActivityCell[],
  config: ActivityMapConfig,
  filledCols: number
): ActivityXLabel[] {
  if (filledCols === 0) return [];
  const wanted = 5;
  const span = Math.max(3, Math.min(7, Math.floor(filledCols / 8)));
  const labels: ActivityXLabel[] = [];

  for (let i = 0; i < wanted; i++) {
    const col = Math.round((i / (wanted - 1)) * (filledCols - 1));
    const isLast = i === wanted - 1;
    const startColumn = isLast ? filledCols - span + 1 : col + 1;
    const cell = cells[col * config.rows];
    if (!cell) continue;

    const previous = labels[labels.length - 1];
    if (!isLast && previous && startColumn < previous.column + span) continue;
    while (
      isLast &&
      labels.length &&
      startColumn < labels[labels.length - 1].column + span
    ) {
      labels.pop();
    }

    labels.push({
      column: Math.max(1, startColumn),
      span,
      label: formatAxisLabel(cell.startMs, config),
      alignEnd: isLast,
    });
  }
  return labels;
}

function pad(value: number): string {
  return String(value).padStart(2, '0');
}

/** Clock time for short windows, a date once the window spans days. */
export function formatAxisLabel(ms: number, config: ActivityMapConfig): string {
  const date = new Date(ms);
  const spanHours = (config.cols * config.rows * config.unitMinutes) / 60;
  const clock = `${pad(date.getHours())}:${pad(date.getMinutes())}`;
  const day = date.toLocaleDateString(undefined, {
    month: 'short',
    day: '2-digit',
  });
  if (spanHours <= 24) return clock;
  if (spanHours <= 24 * 7) return `${day} ${clock}`;
  return day;
}

/** "Sep 01 · 14:06–14:12", the range one cell covers. */
export function formatCellRange(cell: ActivityCell): string {
  const start = new Date(cell.startMs);
  const end = new Date(cell.endMs);
  const day = start.toLocaleDateString(undefined, {
    month: 'short',
    day: '2-digit',
  });
  const clock = (d: Date) => `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  return `${day} · ${clock(start)}–${clock(end)}`;
}
