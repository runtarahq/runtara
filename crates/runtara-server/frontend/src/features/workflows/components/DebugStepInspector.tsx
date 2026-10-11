import { X } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { PayloadExplorer } from '@/shared/components/PayloadExplorer';
import { SectionLabel } from '@/shared/components/section-label';
import { cn } from '@/lib/utils';
import { useExecutionStore } from '@/features/workflows/stores/executionStore';
import { useWorkflowStore } from '@/features/workflows/stores/workflowStore';

/** Debug data counts as present unless it is missing entirely. */
function hasValue(value: unknown) {
  return value !== null && value !== undefined;
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
  // Only the step the run stopped at carries the breakpoint marker; a step
  // parked by a waiting run is `suspended` too but is not at a breakpoint.
  const isSuspendedStep =
    isCurrentBreakpoint && nodeStatus?.status === 'suspended';

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
        'absolute right-2 top-16 z-20 max-h-[calc(100%-5rem)] w-80',
        'rounded-lg border bg-background/95 shadow-lg backdrop-blur',
        'flex flex-col overflow-hidden'
      )}
    >
      {/* Header */}
      <div className="flex shrink-0 items-center justify-between border-b px-3 py-2">
        <div className="flex min-w-0 flex-col">
          <div className="flex items-center gap-1.5">
            <span className="truncate text-xs font-semibold text-foreground">
              {stepName}
            </span>
            {isSuspendedStep && (
              <span className="rounded bg-info/10 px-1 text-[9px] font-medium text-info">
                breakpoint
              </span>
            )}
          </div>
          <span className="text-3xs text-muted-foreground">
            {stepType}
            {nodeStatus?.executionTime !== undefined &&
              ` \u00b7 ${nodeStatus.executionTime < 1000 ? `${nodeStatus.executionTime}ms` : `${(nodeStatus.executionTime / 1000).toFixed(2)}s`}`}
          </span>
        </div>
        <Button
          type="button"
          variant="secondary"
          size="icon"
          className="size-6 shrink-0"
          onClick={() => useWorkflowStore.getState().setSelectedNodeId(null)}
        >
          <X className="size-3.5" />
        </Button>
      </div>

      {/* Content */}
      <div className="flex-1 space-y-3 overflow-y-auto p-3">
        {nodeStatus?.error && (
          <Section label="Error">
            <p className="text-xs text-destructive">{nodeStatus.error}</p>
          </Section>
        )}

        {(hasValue(inputs) || hasValue(outputs)) && (
          <PayloadExplorer
            key={selectedNodeId ?? undefined}
            compact
            ariaLabel={`${stepName} data`}
            maxHeight={320}
            defaultSectionId={hasValue(inputs) ? 'input' : 'output'}
            sections={[
              {
                id: 'input',
                label: isCurrentBreakpoint ? 'Inputs (resolved)' : 'Inputs',
                value: hasValue(inputs) ? inputs : null,
                emptyState: { title: 'No inputs' },
              },
              {
                id: 'output',
                label: 'Outputs',
                value: hasValue(outputs) ? outputs : null,
                emptyState: { title: 'No outputs' },
              },
            ]}
          />
        )}

        {inputs === null && outputs === null && !nodeStatus?.error && (
          <p className="py-4 text-center text-xs italic text-muted-foreground">
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
      <SectionLabel size="sm" as="h4" className="mb-1">
        {label}
      </SectionLabel>
      {children}
    </div>
  );
}
