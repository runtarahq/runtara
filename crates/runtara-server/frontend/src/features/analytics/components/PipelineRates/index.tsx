import { cn } from '@/lib/utils';
import { formatRate, type PipelineRates as Rates } from '../../utils/pipeline';

interface PipelineRatesProps {
  rates: Rates | null;
}

/// Headline throughput across the pipeline.
///
/// `steps` is the one tile that can legitimately have nothing to show while
/// everything else does: `trackEvents` is compile-time, so a deployment of
/// workflows built without it runs perfectly and reports no steps at all. That
/// renders as "not measured" rather than as zero, because a reader who saw
/// `0/s` beside four healthy numbers would reasonably conclude work had
/// stopped.
export function PipelineRates({ rates }: PipelineRatesProps) {
  const tiles: {
    label: string;
    value: number | null;
    tone?: 'good' | 'bad';
    unmeasured?: boolean;
  }[] = [
    { label: 'Offered', value: rates?.offered ?? null },
    {
      label: 'Accepted',
      value: rates?.accepted ?? null,
      tone: rates && rates.accepted < rates.offered ? undefined : 'good',
    },
    {
      label: 'Denied 403',
      value: rates?.denied ?? null,
      tone: rates && rates.denied > 0 ? 'bad' : undefined,
    },
    { label: 'Started', value: rates?.started ?? null },
    { label: 'Finished', value: rates?.finished ?? null, tone: 'good' },
    {
      label: 'Steps',
      value: rates?.steps ?? null,
      unmeasured: rates !== null && rates.steps === null,
    },
  ];

  return (
    <div className="grid grid-cols-2 gap-px overflow-hidden rounded-lg border border-border/40 bg-border/40 sm:grid-cols-3 lg:grid-cols-6">
      {tiles.map((tile) => (
        <div key={tile.label} className="bg-card px-4 py-2.5">
          <div className="text-[10.5px] font-medium uppercase tracking-wider text-muted-foreground">
            {tile.label}
          </div>
          {tile.unmeasured ? (
            <div
              className="text-sm font-medium text-muted-foreground"
              title="These workflows were compiled without step tracking, so no step can be reported. This is not zero throughput."
            >
              not measured
            </div>
          ) : (
            <div
              className={cn(
                'text-xl font-semibold tabular-nums leading-tight',
                tile.tone === 'bad' && 'text-destructive',
                tile.tone === 'good' && 'text-emerald-600 dark:text-emerald-500'
              )}
            >
              {formatRate(tile.value)}
              <span className="ml-1 text-xs font-medium text-muted-foreground">
                /s
              </span>
            </div>
          )}
        </div>
      ))}
    </div>
  );
}
