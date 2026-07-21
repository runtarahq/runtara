import { useCallback, useEffect, useRef, useState } from 'react';
import { useAuth } from 'react-oidc-context';
import { toast } from 'sonner';
import {
  getReportWorkflowInstanceStatus,
  runReportWorkflow,
} from '../../queries';
import {
  ReportWorkflowActionConfig,
  ReportWorkflowInstanceStatus,
} from '../../types';

const POLL_DELAYS_MS = [100, 200, 400, 800, 1500] as const;
const WORKFLOW_ACTION_TIMEOUT_MS = 10 * 60 * 1000;
const TERMINAL_STATUSES = new Set([
  'completed',
  'failed',
  'timeout',
  'cancelled',
]);

export type ReportWorkflowActionPhase =
  | 'submitting'
  | 'running'
  | 'refreshing';

export type ReportWorkflowActionResult = ReportWorkflowInstanceStatus & {
  workflowId: string;
  instanceId: string;
};

export type RunWorkflowActionArgs = {
  key: string;
  action: ReportWorkflowActionConfig;
  row?: Record<string, unknown>;
  value?: unknown;
  fallbackField?: string;
  selectedRows?: Record<string, unknown>[];
};

export function useReportWorkflowAction({
  onCompleted,
}: {
  onCompleted?: (
    result: ReportWorkflowActionResult,
    action: ReportWorkflowActionConfig
  ) => void | Promise<void>;
} = {}) {
  const auth = useAuth();
  const token = auth.user?.access_token;
  const [phases, setPhases] = useState<Map<string, ReportWorkflowActionPhase>>(
    () => new Map()
  );
  const controllers = useRef(new Map<string, AbortController>());

  useEffect(
    () => () => {
      for (const controller of controllers.current.values()) {
        controller.abort();
      }
      controllers.current.clear();
    },
    []
  );

  const run = useCallback(
    async ({
      key,
      action,
      row = {},
      value,
      fallbackField = '',
      selectedRows,
    }: RunWorkflowActionArgs) => {
      controllers.current.get(key)?.abort();
      const controller = new AbortController();
      controllers.current.set(key, controller);
      setActionPhase(setPhases, key, 'submitting');

      try {
        const context = resolveWorkflowActionContext(
          action,
          row,
          value,
          fallbackField,
          selectedRows
        );
        const scheduled = await runReportWorkflow(token ?? '', {
          workflowId: action.workflowId,
          version: action.version ?? undefined,
          context,
        });
        throwIfAborted(controller.signal);
        setActionPhase(setPhases, key, 'running');
        const terminal = isTerminalStatus(scheduled.status)
          ? {
              id: scheduled.instanceId,
              status: scheduled.status.toLowerCase(),
            }
          : await waitForTerminalInstance(
              token ?? '',
              action.workflowId,
              scheduled.instanceId,
              controller.signal
            );
        const result: ReportWorkflowActionResult = {
          ...terminal,
          workflowId: action.workflowId,
          instanceId: scheduled.instanceId,
        };

        if (result.status === 'completed') {
          setActionPhase(setPhases, key, 'refreshing');
          await onCompleted?.(result, action);
          throwIfAborted(controller.signal);
          toast.success(action.successMessage ?? 'Workflow completed');
          return;
        }

        toast.error(`Workflow finished with status ${result.status}`);
        return;
      } catch (error) {
        if (isAbortError(error)) return;
        toast.error(errorMessage(error) ?? 'Failed to run workflow');
      } finally {
        if (controllers.current.get(key) === controller) {
          controllers.current.delete(key);
          setActionPhase(setPhases, key);
        }
      }
    },
    [onCompleted, token]
  );

  const isRunning = useCallback(
    (key: string) => phases.has(key),
    [phases]
  );
  const phase = useCallback((key: string) => phases.get(key), [phases]);

  return { run, isRunning, phase };
}

export function resolveWorkflowActionContext(
  action: ReportWorkflowActionConfig,
  row: Record<string, unknown>,
  value: unknown,
  fallbackField: string,
  selectedRows: Record<string, unknown>[] = []
): unknown {
  const contextConfig = action.context ?? {};
  const mode = contextConfig.mode ?? 'row';
  const rawContext =
    mode === 'selection'
      ? selectedRows
      : mode === 'value'
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

async function waitForTerminalInstance(
  token: string,
  workflowId: string,
  instanceId: string,
  signal: AbortSignal
): Promise<ReportWorkflowInstanceStatus> {
  const deadline = Date.now() + WORKFLOW_ACTION_TIMEOUT_MS;
  let pollIndex = 0;

  while (Date.now() < deadline) {
    const delayMs =
      POLL_DELAYS_MS[Math.min(pollIndex, POLL_DELAYS_MS.length - 1)];
    await delay(delayMs, signal);
    pollIndex += 1;
    const instance = await getReportWorkflowInstanceStatus(
      token,
      workflowId,
      instanceId
    );
    const status = instance.status.toLowerCase();
    if (isTerminalStatus(status)) {
      return { ...instance, status };
    }
  }

  throw new Error(
    'Workflow did not finish before the report action timed out.'
  );
}

function delay(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    const handle = window.setTimeout(() => {
      signal.removeEventListener('abort', onAbort);
      resolve();
    }, ms);
    const onAbort = () => {
      window.clearTimeout(handle);
      reject(new DOMException('Workflow observation cancelled', 'AbortError'));
    };
    signal.addEventListener('abort', onAbort, { once: true });
  });
}

function throwIfAborted(signal: AbortSignal): void {
  if (signal.aborted) {
    throw new DOMException('Workflow observation cancelled', 'AbortError');
  }
}

function isAbortError(error: unknown): boolean {
  return error instanceof DOMException && error.name === 'AbortError';
}

function setActionPhase(
  setPhases: React.Dispatch<
    React.SetStateAction<Map<string, ReportWorkflowActionPhase>>
  >,
  key: string,
  phase?: ReportWorkflowActionPhase
): void {
  setPhases((current) => {
    const next = new Map(current);
    if (phase) next.set(key, phase);
    else next.delete(key);
    return next;
  });
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
