import { useCallback, useState } from 'react';
import { useAuth } from 'react-oidc-context';
import { toast } from 'sonner';
import {
  getReportWorkflowInstanceStatus,
  runReportWorkflow,
} from '../../queries';
import { ReportWorkflowActionConfig } from '../../types';

const POLL_INTERVAL_MS = 1500;
const WORKFLOW_ACTION_TIMEOUT_MS = 10 * 60 * 1000;
const TERMINAL_STATUSES = new Set([
  'completed',
  'failed',
  'timeout',
  'cancelled',
]);

type RunWorkflowActionArgs = {
  key: string;
  action: ReportWorkflowActionConfig;
  row: Record<string, unknown>;
  value: unknown;
  fallbackField: string;
};

export function useReportWorkflowAction({
  onCompleted,
}: {
  onCompleted?: () => void | Promise<void>;
} = {}) {
  const auth = useAuth();
  const token = auth.user?.access_token;
  const [runningKeys, setRunningKeys] = useState<Set<string>>(() => new Set());

  const run = useCallback(
    async ({ key, action, row, value, fallbackField }: RunWorkflowActionArgs) => {
      setRunningKeys((current) => {
        const next = new Set(current);
        next.add(key);
        return next;
      });

      try {
        const context = resolveWorkflowActionContext(
          action,
          row,
          value,
          fallbackField
        );
        const scheduled = await runReportWorkflow(token ?? '', {
          workflowId: action.workflowId,
          version: action.version,
          context,
        });
        const finalStatus = isTerminalStatus(scheduled.status)
          ? scheduled.status
          : await waitForTerminalStatus(
              token ?? '',
              action.workflowId,
              scheduled.instanceId
            );

        if (finalStatus === 'completed') {
          toast.success(action.successMessage ?? 'Workflow completed');
          if (action.reloadBlock) {
            await onCompleted?.();
          }
          return;
        }

        toast.error(`Workflow finished with status ${finalStatus}`);
      } catch (error) {
        toast.error(errorMessage(error) ?? 'Failed to run workflow');
      } finally {
        setRunningKeys((current) => {
          const next = new Set(current);
          next.delete(key);
          return next;
        });
      }
    },
    [onCompleted, token]
  );

  const isRunning = useCallback(
    (key: string) => runningKeys.has(key),
    [runningKeys]
  );

  return { run, isRunning };
}

export function resolveWorkflowActionContext(
  action: ReportWorkflowActionConfig,
  row: Record<string, unknown>,
  value: unknown,
  fallbackField: string
): unknown {
  const contextConfig = action.context ?? {};
  const mode = contextConfig.mode ?? 'row';
  const rawContext =
    mode === 'value'
      ? value
      : mode === 'field'
        ? row[contextConfig.field ?? fallbackField]
        : row;
  const context = rawContext === undefined ? null : rawContext;

  if (!contextConfig.inputKey) {
    return context;
  }

  return {
    [contextConfig.inputKey]: context,
  };
}

function isTerminalStatus(status: string | undefined): boolean {
  return TERMINAL_STATUSES.has(String(status ?? '').toLowerCase());
}

async function waitForTerminalStatus(
  token: string,
  workflowId: string,
  instanceId: string
): Promise<string> {
  const deadline = Date.now() + WORKFLOW_ACTION_TIMEOUT_MS;

  while (Date.now() < deadline) {
    await delay(POLL_INTERVAL_MS);
    const instance = await getReportWorkflowInstanceStatus(
      token,
      workflowId,
      instanceId
    );
    const status = instance.status.toLowerCase();
    if (isTerminalStatus(status)) {
      return status;
    }
  }

  throw new Error('Workflow did not finish before the report action timed out.');
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

function errorMessage(error: unknown): string | null {
  if (!error || typeof error !== 'object') return null;
  const maybeAxios = error as {
    response?: { data?: { error?: string; message?: string } };
    message?: string;
  };
  return (
    maybeAxios.response?.data?.error ??
    maybeAxios.response?.data?.message ??
    maybeAxios.message ??
    null
  );
}
