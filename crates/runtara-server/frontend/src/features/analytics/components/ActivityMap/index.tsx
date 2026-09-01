import { useRef, useState } from 'react';

import { cn } from '@/lib/utils';
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/shared/components/ui/card';
import type { DateRangeOption } from '@/shared/components/date-range-selector';

import {
  ACTIVITY_MAP_CONFIG,
  formatCellRange,
  type ActivityCell,
  type ActivityMap as ActivityMapModel,
} from '../../utils/activity-map';
import { formatNumber } from '../../utils';

/** Keeps the tallest map (seven rows, at 7d) inside a laptop viewport. */
const CELL_MAX_HEIGHT = 14;

interface ActivityMapProps {
  /**
   * The prebuilt grid.
   *
   * Built by the page rather than here so the "busiest interval" it reports and
   * the one on the executions card come from the same reduction over the same
   * cells; two independent reductions could and did disagree.
   */
  map: ActivityMapModel;
  period: DateRangeOption;
  /** The window the grid spans, shown beside the cell grain. */
  windowLabel?: string;
  loading?: boolean;
}

/**
 * Density reads in the brand blue; only failure is coloured semantically.
 *
 * The reference design ramped this in `--success`, GitHub-contribution-graph
 * style, and split every cell containing a failure with a red diagonal. Two
 * changes. Green is a *status* colour in this console, not a brand one, and
 * density is not a status: a dark green square would say "good" about an
 * interval that might be entirely failures. And a whole cell turns red only
 * when the interval failed materially worse than the window did - the diagonal
 * fired on nearly every cell in any real window, which made the marker
 * meaningless and the density underneath it unreadable.
 */
function cellBackground(cell: ActivityCell, markAllFailures: boolean): string {
  if (cell.intensity <= 0) return 'hsl(var(--muted))';

  const volume = `hsl(var(--chart-1) / ${cell.intensity})`;
  if (cell.elevated) return 'hsl(var(--destructive))';
  if (!markAllFailures || cell.failed === 0) return volume;

  // A corner wedge rather than a fill. Every cell containing a failure used to
  // be split half-red, which on any real window meant most of the map: the
  // marker stopped distinguishing anything and the density underneath was
  // unreadable. A notch is findable when you are scanning for it and ignorable
  // when you are not.
  return `linear-gradient(225deg, hsl(var(--destructive)) 0 28%, transparent 28%), ${volume}`;
}

/**
 * Borders only where a fill cannot carry the shape.
 *
 * Outlining every one of several hundred cells drew a grid of hairlines that
 * competed with the fills for attention; the density is easier to read as an
 * uninterrupted field.
 */
function cellBorder(cell: ActivityCell): string | undefined {
  if (cell.intensity <= 0) return '1px solid hsl(var(--border) / 0.6)';
  return undefined;
}

/** "1 execution", not "1 executions" - the map repeats this several hundred times. */
function plural(count: number, noun: string): string {
  return `${formatNumber(count)} ${noun}${count === 1 ? '' : 's'}`;
}

function describeCell(cell: ActivityCell): string {
  if (cell.total === 0) return `${formatCellRange(cell)}: no executions`;
  const parts = [plural(cell.total, 'execution')];
  if (cell.failed > 0) {
    parts.push(
      `${formatNumber(cell.failed)} failed (${(cell.failRate * 100).toFixed(0)}%)`
    );
  }
  if (cell.cancelled > 0)
    parts.push(`${formatNumber(cell.cancelled)} cancelled`);
  return `${formatCellRange(cell)}: ${parts.join(', ')}`;
}

/**
 * Execution density over the window, one square per bucket.
 *
 * Hand-rolled rather than charted: no chart library expresses a grid whose
 * cells carry two independent scales (how much ran, how much of it failed).
 *
 * Keyboard access is a roving tabindex over a single focusable cell, so the map
 * costs one tab stop instead of four hundred, and each cell carries its own
 * `aria-label` - the hover tooltip alone would leave the whole panel unreadable
 * to a screen reader.
 */
export function ActivityMap({
  map,
  period,
  windowLabel,
  loading,
}: ActivityMapProps) {
  const config = ACTIVITY_MAP_CONFIG[period];
  const [active, setActive] = useState<{ row: number; col: number }>({
    row: 0,
    col: 0,
  });
  const [hovered, setHovered] = useState<ActivityCell | null>(null);
  const [markAllFailures, setMarkAllFailures] = useState(false);
  const gridRef = useRef<HTMLDivElement>(null);

  const gridStyle = {
    display: 'grid',
    gridTemplateColumns: `repeat(${map.cols || config.cols}, minmax(0, 1fr))`,
    gap: '2px',
    flex: '1 1 auto',
    minWidth: 0,
  } as const;

  const move = (event: React.KeyboardEvent, row: number, col: number) => {
    const deltas: Record<string, [number, number]> = {
      ArrowRight: [0, 1],
      ArrowLeft: [0, -1],
      ArrowDown: [1, 0],
      ArrowUp: [-1, 0],
    };
    const delta = deltas[event.key];
    if (!delta) return;
    event.preventDefault();
    const nextRow = Math.min(Math.max(row + delta[0], 0), map.rows.length - 1);
    const nextCol = Math.min(
      Math.max(col + delta[1], 0),
      (map.rows[nextRow]?.cells.length ?? 1) - 1
    );
    setActive({ row: nextRow, col: nextCol });
    const selector = `[data-cell="${nextRow}-${nextCol}"]`;
    gridRef.current?.querySelector<HTMLElement>(selector)?.focus();
  };

  // Cells that contain failures but did not clear the elevated threshold -
  // exactly what the toggle reveals, so it says how much it will add.
  const quietFailureCells = map.cellsWithFailures - map.elevatedCells;

  const summary = map.peak
    ? `Busiest ${config.grain}: ${plural(map.peak.total, 'execution')} at ${formatCellRange(map.peak)}.`
    : 'No executions in this window.';

  return (
    <Card className="flex min-h-0 flex-col border-border/40 shadow-none">
      <CardHeader className="flex shrink-0 flex-row items-start justify-between space-y-0 p-4 pb-2">
        <div className="flex flex-col gap-1">
          <CardTitle className="text-base">Activity map</CardTitle>
          <CardDescription>
            One square per {config.grain}
            {windowLabel ? ` · ${windowLabel}` : ''}
          </CardDescription>
        </div>
        {/* "Quieter -> Busier" rather than "None -> Most": cells are binned by
            quartile within the window, so a full-strength square means "busier
            than most intervals here", not "at or near some absolute peak". The
            exact count is in the tooltip and the busiest interval is named in
            text below. */}
        <div className="flex flex-wrap items-center gap-1.5 whitespace-nowrap text-xs text-muted-foreground">
          <span>None</span>
          <span className="sr-only">Quieter</span>
          {[0, 0.22, 0.45, 0.7, 1].map((level) => (
            <span
              key={level}
              className="size-3.5 rounded-[3px]"
              style={{
                background:
                  level === 0
                    ? 'hsl(var(--muted))'
                    : `hsl(var(--chart-1) / ${level})`,
                border:
                  level === 0
                    ? '1px solid hsl(var(--border) / 0.6)'
                    : undefined,
              }}
            />
          ))}
          <span>Busier</span>
          <span className="mx-1.5 h-3.5 w-px bg-border" />
          <span
            className="size-3.5 rounded-[3px]"
            style={{ background: 'hsl(var(--destructive))' }}
          />
          <span>Elevated failures</span>
          {quietFailureCells > 0 ? (
            <>
              <span className="mx-1.5 h-3.5 w-px bg-border" />
              <button
                type="button"
                aria-pressed={markAllFailures}
                onClick={() => setMarkAllFailures((on) => !on)}
                className={cn(
                  'flex items-center gap-1.5 rounded border px-1.5 py-0.5 transition-colors',
                  markAllFailures
                    ? 'border-destructive/40 bg-destructive/10 text-foreground'
                    : 'border-transparent hover:border-border hover:bg-muted'
                )}
                title={`${quietFailureCells} further ${
                  quietFailureCells === 1 ? 'interval' : 'intervals'
                } contain failures below the elevated threshold`}
              >
                <span
                  className="size-3.5 shrink-0 rounded-[3px]"
                  style={{
                    background:
                      'linear-gradient(225deg, hsl(var(--destructive)) 0 28%, transparent 28%), hsl(var(--chart-1) / 0.45)',
                  }}
                />
                <span>All failures ({formatNumber(quietFailureCells)})</span>
              </button>
            </>
          ) : null}
        </div>
      </CardHeader>

      <CardContent className="flex min-h-0 flex-1 flex-col px-4 pb-3 pt-1">
        {loading ? (
          <div className="min-h-0 flex-1 animate-pulse rounded bg-muted" />
        ) : map.rows.length === 0 ? (
          <div className="flex min-h-0 flex-1 items-center justify-center text-sm text-muted-foreground">
            No executions in this window
          </div>
        ) : (
          <div className="flex min-h-0 flex-1 flex-col justify-between gap-2">
            <div
              ref={gridRef}
              role="grid"
              aria-label={`Execution activity, one cell per ${config.grain}`}
              className="flex min-w-[520px] flex-col justify-center gap-[2px]"
              onMouseLeave={() => setHovered(null)}
            >
              {map.rows.map((row, rowIndex) => (
                <div
                  key={row.label}
                  role="row"
                  className="flex items-center gap-2"
                >
                  <span className="w-[46px] shrink-0 text-right text-xs tabular-nums leading-[14px] text-muted-foreground">
                    {row.label}
                  </span>
                  <div style={gridStyle}>
                    {row.cells.map((cell, colIndex) => {
                      const isActive =
                        active.row === rowIndex && active.col === colIndex;
                      return (
                        <button
                          key={cell.key}
                          type="button"
                          role="gridcell"
                          data-cell={`${rowIndex}-${colIndex}`}
                          tabIndex={isActive ? 0 : -1}
                          aria-label={describeCell(cell)}
                          title={describeCell(cell)}
                          onFocus={() => {
                            setActive({ row: rowIndex, col: colIndex });
                            setHovered(cell);
                          }}
                          onMouseEnter={() => setHovered(cell)}
                          onKeyDown={(e) => move(e, rowIndex, colIndex)}
                          className="aspect-square rounded-[2px] outline-none transition-shadow hover:ring-2 hover:ring-foreground focus-visible:ring-2 focus-visible:ring-foreground"
                          style={{
                            background: cellBackground(cell, markAllFailures),
                            border: cellBorder(cell),
                            // Cells are square until that would make a 7-row
                            // map taller than the space it has; then height
                            // wins and they become slightly wide.
                            maxHeight: CELL_MAX_HEIGHT,
                          }}
                        />
                      );
                    })}
                  </div>
                </div>
              ))}
              <div className="flex items-center gap-2 pt-1">
                <span className="w-[46px] shrink-0" />
                <div
                  style={gridStyle}
                  className="text-xs text-muted-foreground"
                  aria-hidden
                >
                  {map.xLabels.map((label) => (
                    <span
                      key={`${label.column}-${label.label}`}
                      style={{
                        gridColumn: `${label.column} / span ${label.span}`,
                        textAlign: label.alignEnd ? 'right' : 'left',
                      }}
                    >
                      {label.label}
                    </span>
                  ))}
                </div>
              </div>
            </div>

            {/* The map's detail is otherwise hover-only. This line carries the
                one fact the grid exists to surface, in text. */}
            <p
              className="shrink-0 border-t pt-1.5 text-xs text-muted-foreground"
              aria-live="polite"
            >
              {hovered ? describeCell(hovered) : summary}
            </p>
          </div>
        )}
      </CardContent>
    </Card>
  );
}
