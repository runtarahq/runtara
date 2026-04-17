import { useCallback, useEffect, useMemo } from 'react';
import { useCustomQuery } from '@/shared/hooks/api';
import { useToken } from '@/shared/hooks';
import { queryKeys } from '@/shared/queries/query-keys';
import { getStepSummaries } from '@/features/scenarios/queries';
import { useTimelineStore } from '@/features/scenarios/stores/timelineStore';
import {
  HierarchicalStep,
  toHierarchicalStep,
  calculateMinTimestamp,
  calculateMaxTimestamp,
} from '@/features/scenarios/types/timeline';
import { RuntimeREST } from '@/shared/queries';
import { createAuthHeaders } from '@/shared/queries/utils';

const ROOT_PAGE_SIZE = 50;
const CHILDREN_PAGE_SIZE = 100;

interface UseHierarchicalTimelineOptions {
  /** Polling interval in ms (default: 10000) */
  refetchInterval?: number;
  /** Whether to enable the queries */
  enabled?: boolean;
}

export function useHierarchicalTimeline(
  scenarioId: string | undefined,
  instanceId: string | undefined,
  options: UseHierarchicalTimelineOptions = {}
) {
  const { refetchInterval = 10000, enabled = true } = options;
  const token = useToken();

  const {
    rootSteps,
    totalRootCount,
    isLoadingRoot,
    rootOffset,
    minTimestamp,
    totalDuration,
    expandedStepIds,
    loadingChildrenIds,
    childrenCache,
    childrenTotalCounts,
    currentScenarioId,
    currentInstanceId,
    setRootSteps,
    appendRootSteps,
    setLoadingRoot,
    toggleExpanded,
    setChildren,
    setLoadingChildren,
    setCurrentInstance,
  } = useTimelineStore();

  // Reset store when scenario or instance changes
  useEffect(() => {
    if (scenarioId !== currentScenarioId || instanceId !== currentInstanceId) {
      setCurrentInstance(scenarioId ?? null, instanceId ?? null);
    }
  }, [
    scenarioId,
    instanceId,
    currentScenarioId,
    currentInstanceId,
    setCurrentInstance,
  ]);

  // Fetch root steps with rootScopesOnly=true
  const rootFilters = useMemo(
    () => ({
      rootScopesOnly: true,
      sortOrder: 'asc' as const,
      limit: ROOT_PAGE_SIZE,
      offset: 0,
    }),
    []
  );

  const {
    data: rootData,
    isFetching: isRootFetching,
    refetch: refetchRoot,
  } = useCustomQuery({
    queryKey: queryKeys.scenarios.stepSummaries(
      scenarioId ?? '',
      instanceId ?? null,
      rootFilters
    ),
    queryFn: (token: string) =>
      getStepSummaries(token, scenarioId!, instanceId!, rootFilters),
    enabled: !!scenarioId && !!instanceId && enabled,
    refetchInterval,
  });

  // Process root steps when data changes
  useEffect(() => {
    if (rootData?.data?.steps) {
      const allSteps = rootData.data.steps;

      // Filter to only root steps (those without parentScopeId)
      // This is a fallback in case the backend doesn't properly filter with rootScopesOnly
      const steps = allSteps.filter((step) => !step.parentScopeId);

      // Use total count from filtered steps if backend returned non-root steps
      const totalCount =
        steps.length !== allSteps.length
          ? steps.length
          : rootData.data.totalCount;

      if (steps.length > 0) {
        const minTs = calculateMinTimestamp(steps);
        const maxTs = calculateMaxTimestamp(steps);

        const hierarchicalSteps = steps.map((step) =>
          toHierarchicalStep(step, 0, minTs)
        );

        setRootSteps(hierarchicalSteps, totalCount, minTs, maxTs);
      } else {
        setRootSteps([], totalCount, 0, 0);
      }
    }
  }, [rootData, setRootSteps]);

  // Update loading state
  useEffect(() => {
    setLoadingRoot(isRootFetching && rootSteps.length === 0);
  }, [isRootFetching, rootSteps.length, setLoadingRoot]);

  // Load more root steps
  const loadMoreRoot = useCallback(async () => {
    if (!scenarioId || !instanceId || !token) return;

    setLoadingRoot(true);

    try {
      const result = await RuntimeREST.api.getStepSummaries(
        scenarioId,
        instanceId,
        {
          rootScopesOnly: true,
          sortOrder: 'asc',
          limit: ROOT_PAGE_SIZE,
          offset: rootOffset,
        },
        createAuthHeaders(token)
      );

      if (result.data?.data?.steps) {
        const newSteps = result.data.data.steps.map((step) =>
          toHierarchicalStep(step, 0, minTimestamp)
        );
        appendRootSteps(newSteps);
      }
    } finally {
      setLoadingRoot(false);
    }
  }, [
    scenarioId,
    instanceId,
    token,
    rootOffset,
    minTimestamp,
    appendRootSteps,
    setLoadingRoot,
  ]);

  // Fetch children for a step
  const fetchChildren = useCallback(
    async (step: HierarchicalStep) => {
      const scopeIdForChildren = step.childrenScopeId;
      if (!scenarioId || !instanceId || !token || !scopeIdForChildren) return;

      // Check if children are already cached
      const cached = childrenCache.get(scopeIdForChildren);
      if (cached && cached.length > 0) {
        return;
      }

      setLoadingChildren(step.stepId, true);

      try {
        // Fetch children using parentScopeId filter
        // The childrenScopeId is the scope that this step creates, children have it as parentScopeId
        const result = await RuntimeREST.api.getStepSummaries(
          scenarioId,
          instanceId,
          {
            parentScopeId: scopeIdForChildren,
            sortOrder: 'asc',
            limit: CHILDREN_PAGE_SIZE,
          },
          createAuthHeaders(token)
        );

        if (result.data?.data?.steps) {
          const childSteps = result.data.data.steps.map((childStep) =>
            toHierarchicalStep(childStep, step.depth + 1, minTimestamp)
          );
          setChildren(
            scopeIdForChildren,
            childSteps,
            result.data.data.totalCount
          );
        }
      } finally {
        setLoadingChildren(step.stepId, false);
      }
    },
    [
      scenarioId,
      instanceId,
      token,
      minTimestamp,
      childrenCache,
      setChildren,
      setLoadingChildren,
    ]
  );

  // Toggle expand and fetch children if needed
  const handleToggleExpand = useCallback(
    async (step: HierarchicalStep) => {
      const isCurrentlyExpanded = expandedStepIds.has(step.stepId);

      // Toggle the expansion state
      toggleExpanded(step.stepId);

      // If expanding and has children capability, fetch children
      if (!isCurrentlyExpanded && step.hasChildren && step.childrenScopeId) {
        await fetchChildren(step);
      }
    },
    [expandedStepIds, toggleExpanded, fetchChildren]
  );

  // Build the flattened list of visible steps (with hierarchy)
  const visibleSteps = useMemo(() => {
    const result: HierarchicalStep[] = [];

    const addStepsRecursively = (steps: HierarchicalStep[], depth: number) => {
      for (const step of steps) {
        // Add current step with updated expansion state
        const isExpanded = expandedStepIds.has(step.stepId);
        const isLoadingChildrenForStep = loadingChildrenIds.has(step.stepId);
        const childrenScopeId = step.childrenScopeId;
        const children = childrenScopeId
          ? childrenCache.get(childrenScopeId)
          : undefined;

        const stepWithState: HierarchicalStep = {
          ...step,
          isExpanded,
          isLoadingChildren: isLoadingChildrenForStep,
          children,
          childrenTotalCount: childrenScopeId
            ? childrenTotalCounts.get(childrenScopeId)
            : undefined,
          depth,
        };

        result.push(stepWithState);

        // If expanded and has children, add them recursively
        if (isExpanded && children && children.length > 0) {
          addStepsRecursively(children, depth + 1);
        }
      }
    };

    addStepsRecursively(rootSteps, 0);
    return result;
  }, [
    rootSteps,
    expandedStepIds,
    loadingChildrenIds,
    childrenCache,
    childrenTotalCounts,
  ]);

  // Calculate stats
  const stats = useMemo(() => {
    const byType: Record<string, number> = {};

    for (const step of visibleSteps) {
      const duration = step.durationMs || 0;

      if (!byType[step.stepType]) {
        byType[step.stepType] = 0;
      }
      byType[step.stepType] += duration;
    }

    return {
      total: totalDuration,
      byType,
      stepCount: visibleSteps.length,
      rootStepCount: rootSteps.length,
    };
  }, [visibleSteps, totalDuration, rootSteps.length]);

  const hasMoreRootSteps = rootSteps.length < totalRootCount;

  return {
    // Data
    visibleSteps,
    rootSteps,
    totalRootCount,
    totalDuration,
    minTimestamp,
    stats,

    // Loading states
    isLoadingRoot: isLoadingRoot || (isRootFetching && rootSteps.length === 0),
    hasMoreRootSteps,

    // Actions
    loadMoreRoot,
    toggleExpand: handleToggleExpand,
    refetch: refetchRoot,

    // Helpers
    isStepExpanded: (stepId: string) => expandedStepIds.has(stepId),
    isLoadingChildren: (stepId: string) => loadingChildrenIds.has(stepId),
    getChildren: (scopeId: string) => childrenCache.get(scopeId),
  };
}
