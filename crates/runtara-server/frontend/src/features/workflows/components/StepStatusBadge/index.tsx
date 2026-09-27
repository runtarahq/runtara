import { Badge } from '@/shared/components/ui/badge';
import { Spinner } from '@/shared/components/ui/spinner';
import { cn } from '@/lib/utils';
import { stepStatusDisplay } from '@/features/workflows/utils/step-status';

/**
 * Status badge for a step summary: Running (spinner), Suspended (a parked
 * step, with a hover explanation), Completed, Failed, or the run's terminal
 * status. Shared by the List, Timeline and debug history views.
 */
export function StepStatusBadge({
  status,
  className,
  spinnerClassName = 'mr-1 size-3',
}: {
  status: string | null | undefined;
  className?: string;
  spinnerClassName?: string;
}) {
  const display = stepStatusDisplay(status);
  return (
    <Badge
      variant={display.badgeVariant}
      className={cn(className)}
      title={display.title}
      data-step-status={(status || '').toLowerCase() || undefined}
    >
      {display.spin && <Spinner className={spinnerClassName} />}
      {display.label}
    </Badge>
  );
}
