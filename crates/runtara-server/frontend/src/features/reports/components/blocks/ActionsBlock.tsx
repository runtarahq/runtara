import { useManagedInputSubmissions } from '@/features/workflows/hooks/useManagedInputSubmissions';
import { CheckCircle2, CircleDot, Wrench } from 'lucide-react';
import { Badge } from '@/shared/components/ui/badge';
import { ActionForm } from '@/features/workflows/components/ActionForm';
import {
  ReportBlockDefinition,
  ReportBlockResult,
  ReportWorkflowAction,
} from '../../types';

type ActionsData = {
  actions?: ReportWorkflowAction[];
  rows?: ReportWorkflowAction[];
};

interface ActionsBlockProps {
  reportId: string;
  block: ReportBlockDefinition;
  result: ReportBlockResult;
  filters: Record<string, unknown>;
  blockFilters: Record<string, unknown>;
  onSubmitted?: () => void | Promise<void>;
}

export function ActionsBlock({
  reportId,
  block,
  result,
  filters,
  blockFilters,
  onSubmitted,
}: ActionsBlockProps) {
  const submissions = useManagedInputSubmissions();
  const forTarget = (action: ReportWorkflowAction) =>
    submissions.inputs.filter(
      (input) =>
        input.request.kind === 'report' &&
        input.request.reportId === reportId &&
        input.request.blockId === block.id &&
        input.request.instanceId === action.instanceId &&
        input.request.requestId === action.actionId
    );
  const data = (result.data ?? {}) as ActionsData;
  const actions = (data.actions ?? data.rows ?? []).filter(
    (action) => !forTarget(action).some((input) => input.state === 'accepted')
  );

  if (actions.length === 0) {
    return (
      <div className="rounded-lg border bg-background p-6 text-sm text-muted-foreground">
        No open actions.
      </div>
    );
  }

  return (
    <div className="space-y-3">
      {actions.map((action) => {
        const isSubmitting = forTarget(action).some(
          (input) => input.state === 'submitting'
        );
        return (
          <div
            key={`${action.instanceId}-${action.actionId}`}
            className="rounded-lg border bg-background p-4"
          >
            <div className="mb-3 flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
              <div className="min-w-0">
                <div className="flex items-center gap-2 text-sm font-medium">
                  <CircleDot className="size-4 text-warning" />
                  <span className="truncate">{action.label}</span>
                </div>
                {action.message ? (
                  <p className="mt-2 text-sm text-muted-foreground">
                    {action.message}
                  </p>
                ) : null}
              </div>
              <div className="flex shrink-0 flex-wrap items-center gap-2">
                <Badge variant="outline" className="gap-1">
                  <Wrench className="size-3" />
                  {action.actionKind}
                </Badge>
                <Badge variant="secondary" className="gap-1">
                  <CheckCircle2 className="size-3" />
                  {action.status}
                </Badge>
              </div>
            </div>
            <div className="report-print-hidden">
              <ActionForm
                key={action.actionId}
                inputSchema={action.inputSchema}
                disabled={isSubmitting}
                submitLabel={block.actions?.submit?.label ?? 'Submit Action'}
                onSubmit={(payload) => {
                  void submissions
                    .submit(
                      {
                        kind: 'report',
                        reportId,
                        blockId: block.id,
                        instanceId: action.instanceId,
                        requestId: action.actionId,
                        payload,
                        filters,
                        blockFilters,
                      },
                      action.label
                    )
                    .then((confirmed) => {
                      if (confirmed)
                        void Promise.resolve()
                          .then(() => onSubmitted?.())
                          .catch(() => {});
                    });
                }}
              />
            </div>
          </div>
        );
      })}
    </div>
  );
}
