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
        ? 'text-green-600'
        : trend === 'down'
          ? 'text-red-600'
          : 'text-gray-600';
    }

    if (title.toLowerCase().includes('error')) {
      return trend === 'up'
        ? 'text-red-600'
        : trend === 'down'
          ? 'text-green-600'
          : 'text-gray-600';
    }

    if (
      title.toLowerCase().includes('duration') ||
      title.toLowerCase().includes('time')
    ) {
      return trend === 'up'
        ? 'text-red-600'
        : trend === 'down'
          ? 'text-green-600'
          : 'text-gray-600';
    }

    return trend === 'up'
      ? 'text-green-600'
      : trend === 'down'
        ? 'text-red-600'
        : 'text-gray-600';
  };

  const formatValue = (val: string | number) => {
    if (typeof val === 'number') {
      return val.toLocaleString();
    }
    return val;
  };

  if (loading) {
    return (
      <Card className="h-full rounded-xl border border-border/40 bg-card px-4 py-3 sm:px-5 sm:py-4 shadow-none">
        <div className="flex h-full flex-col">
          <div className="min-h-[40px] text-sm font-medium text-muted-foreground leading-snug">
            {title}
          </div>
          <div className="flex flex-1 items-center">
            <div className="h-7 w-24 rounded bg-muted animate-pulse" />
          </div>
          <div className="h-5 w-20 rounded bg-muted animate-pulse" />
        </div>
      </Card>
    );
  }

  return (
    <Card className="h-full rounded-xl border border-border/40 bg-card px-4 py-3 sm:px-5 sm:py-4 shadow-none">
      <div className="flex h-full flex-col">
        <div className="min-h-[40px] text-sm font-semibold text-muted-foreground leading-snug">
          {title}
        </div>
        <div className="flex flex-1 items-center">
          <div className="text-2xl font-semibold text-slate-900/90 leading-tight">
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
