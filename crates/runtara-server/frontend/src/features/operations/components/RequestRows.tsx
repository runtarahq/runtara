import { Link } from 'react-router';
import { AlertTriangle, Eye } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { WithTooltip } from '@/shared/components/ui/tooltip';
import { StateValue } from './StateValue';
import type { AttentionRequest } from '../attention-requests';

const rowClass =
  'grid min-h-16 grid-cols-[minmax(0,1fr)_auto] items-center gap-x-4 gap-y-1 px-4 py-3 lg:grid-cols-[minmax(0,1fr)_minmax(0,1.2fr)_minmax(0,1.5fr)_minmax(0,1fr)_6.5rem]';
const textClass = 'col-start-1 min-w-0 truncate lg:col-auto';
const actionsClass =
  'col-start-2 row-span-4 row-start-1 flex shrink-0 items-center justify-end gap-1 lg:col-auto lg:row-span-1 lg:row-auto';
const iconClass =
  'h-8 w-8 shrink-0 rounded-lg p-2 text-muted-foreground hover:bg-primary/10 hover:text-primary';

export function RequestRows({ requests }: { requests: AttentionRequest[] }) {
  return (
    <ul
      className="divide-y divide-border/50"
      aria-label="Requests requiring input"
    >
      {requests.map(({ row, workflowName, due, isOverdue }) => {
        const to = `/operations/runs/${row.workflowId}/${row.instanceId}`;
        const label = row.runLabel || row.instanceId.slice(0, 8);
        return (
          <li key={`${row.instanceId}/${row.requestId}`} className={rowClass}>
            <Link
              className={`${textClass} text-sm font-medium text-primary-text hover:underline`}
              title={label}
              to={to}
            >
              {label}
            </Link>
            <div className={textClass}>
              <p className="truncate text-sm" title={row.label}>
                {row.label}
              </p>
              {row.message && (
                <p
                  className="truncate text-xs text-muted-foreground"
                  title={row.message}
                >
                  {row.message}
                </p>
              )}
            </div>
            <span
              className={`${textClass} text-xs text-muted-foreground`}
              title={workflowName}
            >
              {workflowName}
            </span>
            <div
              className={`${textClass} text-xs ${isOverdue ? 'text-warning' : 'text-muted-foreground'}`}
            >
              {isOverdue && (
                <span className="mb-0.5 flex items-center gap-1">
                  <AlertTriangle aria-hidden="true" className="size-3" />
                  Overdue
                </span>
              )}
              {due ? (
                <>
                  <span className="lg:hidden">Due </span>
                  <StateValue value={due} display={{ kind: 'relative' }} />
                </>
              ) : (
                <span aria-label="No due date">—</span>
              )}
            </div>
            <div className={actionsClass}>
              <WithTooltip label="Review request">
                <Button
                  asChild
                  variant="secondary"
                  size="icon"
                  className={iconClass}
                >
                  <Link aria-label={`Review request for ${label}`} to={to}>
                    <Eye className="size-4" />
                  </Link>
                </Button>
              </WithTooltip>
            </div>
          </li>
        );
      })}
    </ul>
  );
}
