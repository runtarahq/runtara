import { ArrowDownIcon, ArrowUpIcon } from 'lucide-react';

import { cn } from '@/lib/utils';
import { Card, CardContent } from '@/shared/components/ui/card';

interface AnalyticsHeroCardProps {
  label: string;
  value: string;
  /** The metric's signed change, later half of the window against the earlier. */
  change?: number;
  /** Whether that change was an improvement - not which way the number went. */
  trend?: 'up' | 'down' | 'stable';
  /** What the change is measured against, e.g. "previous 15 days". */
  comparisonLabel?: string;
  loading?: boolean;
  children?: React.ReactNode;
}

/**
 * The compact top-of-page KPI tile.
 *
 * Six of these share the top row - the usage figures and the host readings -
 * so each is a label, a value and a line or two of detail. The trend sits
 * beside the value rather than on a row of its own; what it is measured
 * against goes in the tooltip, since the date range picker already names the
 * window on screen.
 *
 * `trend` already encodes whether a move was good, so the arrow and colour come
 * from it rather than from the sign of `change` - a falling duration is an
 * improvement and reads as "up".
 */
export function AnalyticsHeroCard({
  label,
  value,
  change,
  trend,
  comparisonLabel,
  loading = false,
  children,
}: AnalyticsHeroCardProps) {
  const comparison = comparisonLabel ? ` vs ${comparisonLabel}` : '';
  return (
    <Card className="h-full min-w-0 border-border/40 shadow-none">
      <CardContent className="flex h-full flex-col gap-1.5 p-3">
        <div className="truncate text-xs font-medium text-muted-foreground">
          {label}
        </div>
        {loading ? (
          <div className="h-6 w-24 animate-pulse rounded bg-muted" />
        ) : (
          <div className="flex flex-wrap items-baseline gap-x-2 gap-y-0.5">
            {/* Values are foreground, never a status hue. Colour on this page
                is reserved for things that mean something: the trend arrow,
                the failure red in the map. */}
            <div className="text-xl font-semibold tabular-nums leading-none tracking-tight">
              {value}
            </div>
            {/* Nothing is drawn for a move inside the noise threshold. */}
            {change !== undefined && trend && trend !== 'stable' ? (
              <div
                title={`${Math.abs(change).toFixed(0)}%${comparison}`}
                className={cn(
                  'flex items-center gap-0.5 text-xs font-medium',
                  // The arrow follows the number; the colour says whether that
                  // is welcome. A duration falling 30% is a down arrow in green.
                  trend === 'up' ? 'text-success' : 'text-destructive'
                )}
              >
                {change > 0 ? (
                  <ArrowUpIcon className="size-3.5" aria-hidden />
                ) : (
                  <ArrowDownIcon className="size-3.5" aria-hidden />
                )}
                <span>
                  {`${Math.abs(change).toFixed(0)}%`}
                  <span className="sr-only">{comparison}</span>
                </span>
              </div>
            ) : null}
          </div>
        )}
        {children}
      </CardContent>
    </Card>
  );
}
