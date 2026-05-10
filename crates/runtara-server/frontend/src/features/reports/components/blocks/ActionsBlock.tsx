import { useMemo, useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { CheckCircle2, CircleDot, Wrench } from 'lucide-react';
import { Badge } from '@/shared/components/ui/badge';
import { useCustomMutation } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { ActionForm } from '@/features/workflows/components/ActionForm';
import { submitReportWorkflowAction } from '../../queries';
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
  const queryClient = useQueryClient();
  const [submittingActionId, setSubmittingActionId] = useState<string | null>(
    null
  );
  const [submittedActionIds, setSubmittedActionIds] = useState<Set<string>>(
    () => new Set()
  );
  const mutation = useCustomMutation<
    void,
    {
      actionId: string;
      payload: Record<string, unknown>;
    }
  >({
    mutationFn: (token, request) =>
      submitReportWorkflowAction(token, {
        reportId,
        blockId: block.id,
        actionId: request.actionId,
        payload: request.payload,
        filters,
        blockFilters,
      }),
    onSuccess: async (_data, variables) => {
      setSubmittedActionIds((current) => {
        const next = new Set(current);
        next.add(variables.actionId);
        return next;
      });
      await queryClient.invalidateQueries({
        queryKey: queryKeys.reports.byId(reportId),
      });
      await onSubmitted?.();
    },
    onSettled: () => setSubmittingActionId(null),
  });

  const data = (result.data ?? {}) as ActionsData;
  const allActions = data.actions ?? data.rows ?? [];
  const actions = useMemo(
    () =>
      allActions.filter((action) => !submittedActionIds.has(action.actionId)),
    [allActions, submittedActionIds]
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
        const isSubmitting =
          mutation.isPending && submittingActionId === action.actionId;
        return (
          <div
            key={`${action.instanceId}-${action.actionId}`}
            className="rounded-lg border bg-background p-4"
          >
            <div className="mb-3 flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
              <div className="min-w-0">
                <div className="flex items-center gap-2 text-sm font-medium">
                  <CircleDot className="h-4 w-4 text-amber-600" />
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
                  <Wrench className="h-3 w-3" />
                  {action.actionKind}
                </Badge>
                <Badge variant="secondary" className="gap-1">
                  <CheckCircle2 className="h-3 w-3" />
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
                  setSubmittingActionId(action.actionId);
                  mutation.mutate({
                    actionId: action.actionId,
                    payload,
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
