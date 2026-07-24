import { cn } from '@/lib/utils';
import { ArrowDownIcon, ArrowUpIcon } from 'lucide-react';
import { Card } from '@/shared/components/ui/card';

interface MetricCardProps {
  title: string;
  value: string | number;
  change?: number;
  trend?: 'up' | 'down' | 'stable';
  loading?: boolean;
  format?: 'number' | 'percentage' | 'duration' | 'bytes';
}

export function MetricCard({
  title,
  value,
  change,
  trend,
  loading = false,
}: MetricCardProps) {
  const getTrendColor = () => {
    if (!trend) return '';

    if (
      title.toLowerCase().includes('success') ||
      title.toLowerCase().includes('rate')
    ) {
      return trend === 'up'
        ? 'text-success'
        : trend === 'down'
          ? 'text-destructive'
          : 'text-muted-foreground';
    }

    if (title.toLowerCase().includes('error')) {
      return trend === 'up'
        ? 'text-destructive'
        : trend === 'down'
          ? 'text-success'
          : 'text-muted-foreground';
    }

    if (
      title.toLowerCase().includes('duration') ||
      title.toLowerCase().includes('time')
    ) {
      return trend === 'up'
        ? 'text-destructive'
        : trend === 'down'
          ? 'text-success'
          : 'text-muted-foreground';
    }

    return trend === 'up'
      ? 'text-success'
      : trend === 'down'
        ? 'text-destructive'
        : 'text-muted-foreground';
  };

  const formatValue = (val: string | number) => {
    if (typeof val === 'number') {
      return val.toLocaleString();
    }
    return val;
  };

  if (loading) {
    return (
      <Card className="h-full rounded-lg border border-border/40 bg-card px-4 py-3 shadow-none sm:px-5 sm:py-4">
        <div className="flex h-full flex-col">
          <div className="min-h-[40px] text-sm font-medium leading-snug text-muted-foreground">
            {title}
          </div>
          <div className="flex flex-1 items-center">
            <div className="h-7 w-24 animate-pulse rounded bg-muted" />
          </div>
          <div className="h-5 w-20 animate-pulse rounded bg-muted" />
        </div>
      </Card>
    );
  }

  return (
    <Card className="h-full rounded-lg border border-border/40 bg-card px-4 py-3 shadow-none sm:px-5 sm:py-4">
      <div className="flex h-full flex-col">
        <div className="min-h-[40px] text-sm font-semibold leading-snug text-muted-foreground">
          {title}
        </div>
        <div className="flex flex-1 items-center">
          <div className="text-2xl font-semibold leading-tight text-foreground">
            {formatValue(value)}
          </div>
        </div>
        {change !== undefined ? (
          <div
            className={cn(
              'flex items-center gap-2 text-sm font-medium',
              getTrendColor()
            )}
          >
            {trend === 'up' && <ArrowUpIcon className="h-4 w-4" />}
            {trend === 'down' && <ArrowDownIcon className="h-4 w-4" />}
            <span>
              {`${trend === 'down' ? '-' : '+'}${Math.abs(change).toFixed(1)}%`}
            </span>
          </div>
        ) : (
          <div className="h-5" />
        )}
      </div>
    </Card>
  );
}
