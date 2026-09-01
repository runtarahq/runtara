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
 * The oversized top-of-page KPI.
 *
 * Replaces `shared/components/metric-card`, which despite living in `shared`
 * had exactly one consumer - this page - and has been deleted with it. Six
 * equally weighted cards gave the page no focal point; three of unequal weight
 * put the count and the success rate where the eye lands first.
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
  return (
    <Card className="h-full border-border/40 shadow-none">
      <CardContent className="flex h-full flex-col gap-1 p-3.5">
        <div className="text-sm font-medium text-muted-foreground">{label}</div>
        {loading ? (
          <div className="h-9 w-32 animate-pulse rounded bg-muted" />
        ) : (
          // Values are foreground, never a status hue. Colour on this page is
          // reserved for things that mean something: the trend arrow, the
          // failure red in the map. A giant green number reads as a judgement
          // the figure has not earned.
          <div className="text-[1.75rem] font-semibold tabular-nums leading-none tracking-tight xl:text-[2.25rem]">
            {value}
          </div>
        )}
        {/* Nothing is drawn for a move inside the noise threshold. "0.0% vs
            earlier half" is a row of pixels that tells the reader nothing, and
            it appeared on most cards most of the time. */}
        {change !== undefined && trend && trend !== 'stable' && !loading ? (
          <div
            className={cn(
              'flex items-center gap-1.5 text-sm font-medium',
              // The arrow follows the number; the colour says whether that is
              // welcome. A duration falling 30% is a down arrow in green.
              trend === 'up' ? 'text-success' : 'text-destructive'
            )}
          >
            {change > 0 ? (
              <ArrowUpIcon className="size-4" />
            ) : (
              <ArrowDownIcon className="size-4" />
            )}
            <span>
              {`${Math.abs(change).toFixed(0)}%`}
              {comparisonLabel ? (
                <span className="font-normal text-muted-foreground">
                  {` vs ${comparisonLabel}`}
                </span>
              ) : null}
            </span>
          </div>
        ) : (
          <div className="h-5" />
        )}
        {children}
      </CardContent>
    </Card>
  );
}
