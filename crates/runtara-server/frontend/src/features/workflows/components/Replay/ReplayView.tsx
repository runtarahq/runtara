/**
 * Graph Replay container for the invocation view. Owns the replay clock +
 * time-map and feeds a derived frame to the read-only auto-laid-out graph.
 * Handles loading / empty (track_events off) / no-graph states and the node
 * inspector. Switching pacing preserves the current playhead (model) time.
 */
import { useEffect, useMemo, useRef, useState } from 'react';
import { GitBranch, Loader2, Radio } from 'lucide-react';
import { useReplayModel } from './useReplayModel';
import { buildTimeMap, type ReplayPacing } from './timeMap';
import { useReplayClock } from './useReplayClock';
import { deriveFrame } from './deriveFrame';
import { ReplayGraph } from './ReplayGraph';
import { ReplayTransport } from './ReplayTransport';
import { ReplayInspector } from './ReplayInspector';
import type { ReplayFrame } from './types';

const EMPTY_FRAME: ReplayFrame = {
  t: 0,
  nodeStates: new Map(),
  nodeIterations: new Map(),
  activeEdges: new Set(),
  runningCount: 0,
};

interface ReplayViewProps {
  workflowId: string;
  instanceId: string;
}

export function ReplayView({ workflowId, instanceId }: ReplayViewProps) {
  const { model, isLoading, isError, error, hasEvents, truncated } = useReplayModel(
    workflowId,
    instanceId
  );

  const [pacing, setPacing] = useState<ReplayPacing>('even');
  const [compressIdle, setCompressIdle] = useState(true);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);

  const timeMap = useMemo(
    () =>
      model
        ? buildTimeMap(model, { pacing, compressIdle })
        : null,
    [model, pacing, compressIdle]
  );

  const clock = useReplayClock(timeMap?.displayEnd ?? 1);
  const modelT = timeMap ? timeMap.toModel(clock.displayT) : 0;

  const frame = useMemo(
    () => (model ? deriveFrame(model, modelT) : EMPTY_FRAME),
    [model, modelT]
  );

  // Preserve the playhead's model time when pacing/compression changes.
  const pendingSeekModelT = useRef<number | null>(null);
  const changePacing = (p: ReplayPacing) => {
    pendingSeekModelT.current = modelT;
    setPacing(p);
  };
  const changeCompress = (v: boolean) => {
    pendingSeekModelT.current = modelT;
    setCompressIdle(v);
  };
  useEffect(() => {
    if (timeMap && pendingSeekModelT.current != null) {
      clock.seek(timeMap.toDisplay(pendingSeekModelT.current));
      pendingSeekModelT.current = null;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [timeMap]);

  // Reset selection when switching instances.
  useEffect(() => {
    setSelectedNodeId(null);
  }, [instanceId]);

  if (isLoading) {
    return (
      <div className="flex h-[520px] items-center justify-center">
        <Loader2 className="h-8 w-8 animate-spin text-muted-foreground" />
      </div>
    );
  }

  if (isError) {
    return (
      <EmptyState
        icon={<GitBranch className="h-6 w-6 text-muted-foreground" />}
        title="Couldn't load the replay"
        subtitle={error instanceof Error ? error.message : 'Please try again.'}
      />
    );
  }

  if (!model || model.nodeIds.length === 0) {
    return (
      <EmptyState
        icon={<GitBranch className="h-6 w-6 text-muted-foreground" />}
        title="No graph to replay"
        subtitle="The version this run executed has no renderable steps."
      />
    );
  }

  if (!hasEvents) {
    return (
      <EmptyState
        icon={<Radio className="h-6 w-6 text-muted-foreground" />}
        title="Nothing to replay"
        subtitle="Event tracking was off for this run, so there are no step events to animate. Enable step-event tracking on the workflow version to replay future runs."
      />
    );
  }

  const selectedState = selectedNodeId
    ? frame.nodeStates.get(selectedNodeId) ?? 'idle'
    : 'idle';

  return (
    <div className="flex h-[560px] flex-col gap-2" data-testid="replay-view">
      <ReplayTransport
        clock={clock}
        displayEnd={timeMap?.displayEnd ?? 1}
        modelT={modelT}
        totalModelMs={model.rawTEnd}
        pacing={pacing}
        onPacingChange={changePacing}
        compressIdle={compressIdle}
        onToggleCompress={changeCompress}
        runningCount={frame.runningCount}
        gaps={timeMap?.gaps ?? []}
      />

      {truncated && (
        <p className="text-[11px] text-amber-600 dark:text-amber-400">
          This run has a very large number of steps — the replay shows the first
          batch only.
        </p>
      )}

      <div className="relative min-h-0 flex-1 overflow-hidden rounded-md border bg-card">
        <ReplayGraph
          model={model}
          frame={frame}
          selectedNodeId={selectedNodeId}
          onSelectNode={setSelectedNodeId}
        />
        {selectedNodeId && (
          <ReplayInspector
            model={model}
            workflowId={workflowId}
            instanceId={instanceId}
            nodeId={selectedNodeId}
            state={selectedState}
            onClose={() => setSelectedNodeId(null)}
          />
        )}
      </div>
    </div>
  );
}

function EmptyState({
  icon,
  title,
  subtitle,
}: {
  icon: React.ReactNode;
  title: string;
  subtitle: string;
}) {
  return (
    <div className="flex h-[520px] flex-col items-center justify-center p-12 text-center">
      <div className="mb-3 inline-flex h-12 w-12 items-center justify-center rounded-full bg-muted">
        {icon}
      </div>
      <p className="mb-1 text-sm text-muted-foreground">{title}</p>
      <p className="max-w-md text-xs text-muted-foreground">{subtitle}</p>
    </div>
  );
}
