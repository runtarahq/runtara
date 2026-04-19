import { useState } from 'react';
import { X, ChevronDown, ChevronRight } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { cn } from '@/lib/utils';
import { useExecutionStore } from '@/features/workflows/stores/executionStore';
import { useWorkflowStore } from '@/features/workflows/stores/workflowStore';
import { ExecutionStatus } from '@/generated/RuntaraRuntimeApi';

function JsonBlock({
  data,
  defaultOpen = false,
}: {
  data: any;
  defaultOpen?: boolean;
}) {
  const [open, setOpen] = useState(defaultOpen);
  const text = typeof data === 'string' ? data : JSON.stringify(data, null, 2);
  const lines = text?.split('\n').length ?? 0;
  const isLong = lines > 3;

  if (data === undefined || data === null) {
    return <span className="text-xs text-muted-foreground italic">empty</span>;
  }

  if (!isLong) {
    return (
      <pre className="text-xs bg-muted/50 rounded px-2 py-1.5 overflow-x-auto whitespace-pre-wrap break-all text-foreground/80">
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
        {open ? (
          <ChevronDown className="h-3 w-3" />
        ) : (
          <ChevronRight className="h-3 w-3" />
        )}
        {open ? 'Collapse' : `Expand (${lines} lines)`}
      </button>
      {open && (
        <pre className="text-xs bg-muted/50 rounded px-2 py-1.5 mt-1 overflow-x-auto max-h-64 overflow-y-auto whitespace-pre-wrap break-all text-foreground/80">
          {text}
        </pre>
      )}
    </div>
  );
}

export function DebugStepInspector() {
  const selectedNodeId = useWorkflowStore((s) => s.selectedNodeId);
  const selectedNode = useWorkflowStore((s) =>
    s.nodes.find((n) => n.id === selectedNodeId)
  );
  const stepData = useExecutionStore((s) =>
    selectedNodeId ? s.stepDebugData.get(selectedNodeId) : undefined
  );
  const nodeStatus = useExecutionStore((s) =>
    selectedNodeId ? s.nodeExecutionStatus.get(selectedNodeId) : undefined
  );
  const breakpointHit = useExecutionStore((s) => s.breakpointHit);

  if (!selectedNodeId || !selectedNode) {
    return null;
  }

  const stepName = (selectedNode.data as any)?.name || selectedNodeId;
  const stepType = (selectedNode.data as any)?.stepType || '';
  const isCurrentBreakpoint = breakpointHit?.stepId === selectedNodeId;
  const isSuspendedStep = nodeStatus?.status === ExecutionStatus.Suspended;

  // Determine what inputs/outputs to show:
  // - Current breakpoint step: inputs from breakpointHit.inputs (resolved values about to be processed)
  // - Previously completed steps: inputs from stepDebugData, outputs from stepsContext or stepDebugData
  const contextEntry = breakpointHit?.stepsContext?.[selectedNodeId];

  const inputs = isCurrentBreakpoint
    ? (breakpointHit?.inputs ?? null)
    : (stepData?.inputs ?? null);

  const outputs = stepData?.outputs ?? contextEntry?.outputs ?? null;

  return (
    <div
      className={cn(
        'absolute right-2 top-16 z-20 w-80 max-h-[calc(100%-5rem)]',
        'bg-background/95 backdrop-blur border rounded-lg shadow-lg',
        'flex flex-col overflow-hidden'
      )}
    >
      {/* Header */}
      <div className="flex items-center justify-between px-3 py-2 border-b shrink-0">
        <div className="flex flex-col min-w-0">
          <div className="flex items-center gap-1.5">
            <span className="text-xs font-semibold text-foreground truncate">
              {stepName}
            </span>
            {isSuspendedStep && (
              <span className="text-[9px] font-medium text-blue-600 bg-blue-50 px-1 rounded">
                breakpoint
              </span>
            )}
          </div>
          <span className="text-[10px] text-muted-foreground">
            {stepType}
            {nodeStatus?.executionTime !== undefined &&
              ` \u00b7 ${nodeStatus.executionTime < 1000 ? `${nodeStatus.executionTime}ms` : `${(nodeStatus.executionTime / 1000).toFixed(2)}s`}`}
          </span>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="h-6 w-6 shrink-0"
          onClick={() => useWorkflowStore.getState().setSelectedNodeId(null)}
        >
          <X className="h-3.5 w-3.5" />
        </Button>
      </div>

      {/* Content */}
      <div className="flex-1 overflow-y-auto p-3 space-y-3">
        {nodeStatus?.error && (
          <Section label="Error">
            <p className="text-xs text-destructive">{nodeStatus.error}</p>
          </Section>
        )}

        {inputs !== null && inputs !== undefined && (
          <Section
            label={isCurrentBreakpoint ? 'Step Inputs (resolved)' : 'Inputs'}
          >
            <JsonBlock data={inputs} defaultOpen />
          </Section>
        )}

        {outputs !== null && outputs !== undefined && (
          <Section label="Outputs">
            <JsonBlock data={outputs} defaultOpen />
          </Section>
        )}

        {inputs === null && outputs === null && !nodeStatus?.error && (
          <p className="text-xs text-muted-foreground italic text-center py-4">
            No debug data available for this step
          </p>
        )}
      </div>
    </div>
  );
}

function Section({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div>
      <h4 className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground mb-1">
        {label}
      </h4>
      {children}
    </div>
  );
}
