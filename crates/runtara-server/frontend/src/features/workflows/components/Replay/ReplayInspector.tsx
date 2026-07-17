/**
 * Replay node-detail panel: shows a selected step's recorded inputs / outputs /
 * error / timing for the run being replayed. Mirrors DebugStepInspector's look
 * but reads from the replay model + a lazy per-step fetch (summaries elide large
 * payloads), so it never touches the editor's live-execution stores.
 */
import { useState } from 'react';
import { ChevronDown, ChevronRight, X } from 'lucide-react';
import { cn } from '@/lib/utils';
import { Button } from '@/shared/components/ui/button';
import { useCustomQuery } from '@/shared/hooks/api';
import { fetchStepDetail } from './useReplayModel';
import type { ReplayModel, ReplayNodeState } from './types';

function JsonBlock({ data, defaultOpen = false }: { data: unknown; defaultOpen?: boolean }) {
  const [open, setOpen] = useState(defaultOpen);
  if (data === undefined || data === null) {
    return <span className="text-xs italic text-muted-foreground">empty</span>;
  }
  const text = typeof data === 'string' ? data : JSON.stringify(data, null, 2);
  const lines = text?.split('\n').length ?? 0;
  if (lines <= 3) {
    return (
      <pre className="overflow-x-auto whitespace-pre-wrap break-all rounded bg-muted/50 px-2 py-1.5 text-xs text-foreground/80">
        {text}
      </pre>
    );
  }
  return (
    <div>
      <button
        type="button"
        className="flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
        onClick={() => setOpen(!open)}
      >
        {open ? <ChevronDown className="h-3 w-3" /> : <ChevronRight className="h-3 w-3" />}
        {open ? 'Collapse' : `Expand (${lines} lines)`}
      </button>
      {open && (
        <pre className="mt-1 max-h-64 overflow-y-auto overflow-x-auto whitespace-pre-wrap break-all rounded bg-muted/50 px-2 py-1.5 text-xs text-foreground/80">
          {text}
        </pre>
      )}
    </div>
  );
}

function Section({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div>
      <h4 className="mb-1 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
        {label}
      </h4>
      {children}
    </div>
  );
}

const STATE_LABEL: Record<ReplayNodeState, string> = {
  idle: 'not reached',
  running: 'running',
  done: 'completed',
  failed: 'failed',
  suspended: 'suspended',
  skipped: 'not executed',
};

interface ReplayInspectorProps {
  model: ReplayModel;
  workflowId: string;
  instanceId: string;
  nodeId: string;
  state: ReplayNodeState;
  onClose: () => void;
}

function formatMs(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)}ms`;
  return `${(ms / 1000).toFixed(2)}s`;
}

export function ReplayInspector({
  model,
  workflowId,
  instanceId,
  nodeId,
  state,
  onClose,
}: ReplayInspectorProps) {
  const node = model.nodes.get(nodeId);
  const insts = model.instancesByStep.get(nodeId) ?? [];
  const first = insts[0];
  const iters = model.childInstancesByStep.get(nodeId);

  const detail = useCustomQuery({
    queryKey: ['workflows', 'replay', 'stepDetail', workflowId, instanceId, nodeId],
    queryFn: (token: string) => fetchStepDetail(token, workflowId, instanceId, nodeId),
    enabled: state !== 'skipped',
    staleTime: 60_000,
  });

  const inputs = detail.data?.inputs;
  const outputs = detail.data?.outputs;
  const error = detail.data?.error;
  const durationMs = first ? first.rawEndT - first.startT : undefined;

  return (
    <div
      className={cn(
        'absolute right-2 top-2 z-20 flex max-h-[calc(100%-1rem)] w-80 flex-col overflow-hidden',
        'rounded-lg border bg-background/95 shadow-lg backdrop-blur'
      )}
      data-testid="replay-inspector"
    >
      <div className="flex shrink-0 items-center justify-between border-b px-3 py-2">
        <div className="flex min-w-0 flex-col">
          <span className="truncate text-xs font-semibold text-foreground">
            {node?.name ?? nodeId}
          </span>
          <span className="text-[10px] text-muted-foreground">
            {node?.stepType}
            {' · '}
            {STATE_LABEL[state]}
            {durationMs !== undefined && ` · ${formatMs(durationMs)}`}
          </span>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="h-6 w-6 shrink-0"
          onClick={onClose}
          aria-label="Close inspector"
        >
          <X className="h-3.5 w-3.5" />
        </Button>
      </div>

      <div className="flex-1 space-y-3 overflow-y-auto p-3">
        {state === 'skipped' ? (
          <p className="py-4 text-center text-xs italic text-muted-foreground">
            This step was not executed in this run.
          </p>
        ) : (
          <>
            {iters && iters.length > 0 && (
              <Section label="Iterations">
                <p className="text-xs text-foreground/80">
                  {iters.length} recorded execution{iters.length === 1 ? '' : 's'} in
                  nested scopes
                </p>
              </Section>
            )}
            {detail.isLoading && (
              <p className="py-4 text-center text-xs italic text-muted-foreground">
                Loading recorded data…
              </p>
            )}
            {error !== undefined && error !== null && (
              <Section label="Error">
                <p className="text-xs text-destructive">
                  {typeof error === 'string' ? error : JSON.stringify(error)}
                </p>
              </Section>
            )}
            {inputs !== undefined && inputs !== null && (
              <Section label="Inputs">
                <JsonBlock data={inputs} defaultOpen />
              </Section>
            )}
            {outputs !== undefined && outputs !== null && (
              <Section label="Outputs">
                <JsonBlock data={outputs} defaultOpen />
              </Section>
            )}
            {!detail.isLoading &&
              inputs == null &&
              outputs == null &&
              (error == null) && (
                <p className="py-4 text-center text-xs italic text-muted-foreground">
                  No recorded data for this step.
                </p>
              )}
          </>
        )}
      </div>
    </div>
  );
}
