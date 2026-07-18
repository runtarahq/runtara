/**
 * React Query hook: fetch everything a replay needs and fold it into a
 * {@link ReplayModel}.
 *
 *  1. the instance record  → `usedVersion` (the graph it actually ran) + status
 *  2. that version's graph  → React Flow nodes/edges (reuses `getWorkflowWorkflow`)
 *  3. all step summaries    → paired [start,end] intervals per step execution
 *
 * The model + a shared clock drive both the Graph and Timeline replay views.
 */
import { useMemo } from 'react';
import type { Edge, Node } from '@xyflow/react';
import { useCustomQuery } from '@/shared/hooks/api';
import {
  getStepSummaries,
  getWorkflowInstance,
  getWorkflowWorkflow,
  type WorkflowInstanceWithMetadata,
} from '@/features/workflows/queries';
import { NODE_TYPES } from '@/features/workflows/config/workflow';
import { RuntimeREST } from '@/shared/queries';
import { createAuthHeaders } from '@/shared/queries/utils';
import {
  buildReplayModel,
  type ReplayGraphInput,
  type StepSummaryLike,
} from './buildReplayModel';
import type { ReplayModel } from './types';

/** Node types that are chrome, not workflow steps — excluded from the replay DAG. */
const NON_STEP_NODE_TYPES = new Set<string>([
  NODE_TYPES.CreateNode,
  NODE_TYPES.NoteNode,
  NODE_TYPES.StartIndicatorNode,
]);

const SUMMARY_PAGE_SIZE = 1000;
const MAX_SUMMARY_PAGES = 25; // safety cap: 25k executions

export interface ReplayModelResult {
  model: ReplayModel;
  instance: WorkflowInstanceWithMetadata;
  /** True when the summary set was truncated at the page cap. */
  truncated: boolean;
}

/**
 * Reduce the editor's React Flow node/edge set (which explodes Split/While
 * subgraphs into parent-scoped children) down to the top-level step DAG. Each
 * composite step stays a single node; its iterations surface as a counter.
 */
export function graphToReplayInput(nodes: Node[], edges: Edge[]): ReplayGraphInput {
  const kept = nodes.filter(
    (n) => !n.parentId && !NON_STEP_NODE_TYPES.has(n.type ?? '')
  );
  const keptIds = new Set(kept.map((n) => n.id));

  const replayNodes = kept.map((n) => {
    const data = (n.data ?? {}) as { stepType?: string; name?: string };
    return {
      id: n.id,
      stepType: data.stepType ?? n.type ?? 'Agent',
      name: data.name ?? n.id,
    };
  });

  const replayEdges = edges
    .filter((e) => keptIds.has(e.source) && keptIds.has(e.target))
    .map((e) => ({
      id: e.id,
      source: e.source,
      target: e.target,
      sourceHandle: e.sourceHandle,
    }));

  return { nodes: replayNodes, edges: replayEdges };
}

async function fetchAllSummaries(
  token: string,
  workflowId: string,
  instanceId: string
): Promise<{ summaries: StepSummaryLike[]; truncated: boolean }> {
  const all: StepSummaryLike[] = [];
  let offset = 0;
  let truncated = false;
  for (let page = 0; page < MAX_SUMMARY_PAGES; page++) {
    const resp = await getStepSummaries(token, workflowId, instanceId, {
      sortOrder: 'asc',
      limit: SUMMARY_PAGE_SIZE,
      offset,
    });
    const steps = (resp?.data?.steps ?? []) as StepSummaryLike[];
    all.push(...steps);
    const total = resp?.data?.totalCount ?? all.length;
    offset += steps.length;
    if (steps.length < SUMMARY_PAGE_SIZE || all.length >= total) break;
    if (page === MAX_SUMMARY_PAGES - 1) truncated = true;
  }
  return { summaries: all, truncated };
}

export function useReplayModel(
  workflowId: string | undefined,
  instanceId: string | undefined,
  options: { enabled?: boolean } = {}
) {
  const query = useCustomQuery<ReplayModelResult>({
    queryKey: ['workflows', 'replay', workflowId ?? '', instanceId ?? ''],
    enabled: !!workflowId && !!instanceId && (options.enabled ?? true),
    staleTime: 30_000,
    queryFn: async (token: string) => {
      const instance = await getWorkflowInstance(token, workflowId!, instanceId!);
      const version = instance.usedVersion;

      // The graph the instance actually ran (not the current editor draft).
      const wf = await getWorkflowWorkflow(token, workflowId!, version);
      const graphInput = graphToReplayInput(
        (wf.data.nodes ?? []) as Node[],
        (wf.data.edges ?? []) as Edge[]
      );

      const { summaries, truncated } = await fetchAllSummaries(
        token,
        workflowId!,
        instanceId!
      );

      const model = buildReplayModel(summaries, graphInput, {
        instanceStatus: instance.status,
      });
      return { model, instance, truncated };
    },
  });

  // Stable references so downstream memo/effects don't churn each poll.
  const model = query.data?.model;
  const modelSignature = useMemo(
    () =>
      model
        ? `${model.nodeIds.length}:${model.instances.length}:${model.tEnd}`
        : 'none',
    [model]
  );

  return {
    ...query,
    model,
    modelSignature,
    instance: query.data?.instance,
    truncated: query.data?.truncated ?? false,
    hasEvents: model?.hasEvents ?? false,
  };
}

/**
 * Lazily fetch the full recorded inputs/outputs/error for one step execution
 * (the summaries endpoint elides large payloads). Used by the node inspector.
 */
export async function fetchStepDetail(
  token: string,
  workflowId: string,
  instanceId: string,
  stepId: string
): Promise<StepSummaryLike | null> {
  const resp = await RuntimeREST.api.getStepSummaries(
    workflowId,
    instanceId,
    { stepId, sortOrder: 'asc', limit: 1 } as never,
    createAuthHeaders(token)
  );
  const steps = (resp.data?.data?.steps ?? []) as StepSummaryLike[];
  return steps[0] ?? null;
}
