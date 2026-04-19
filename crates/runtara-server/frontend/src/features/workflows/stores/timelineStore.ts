import { create } from 'zustand';
import { devtools } from 'zustand/middleware';
import { immer } from 'zustand/middleware/immer';
import { enableMapSet } from 'immer';
import { HierarchicalStep } from '../types/timeline';

// Enable Map and Set support in Immer
enableMapSet();

interface TimelineState {
  // Root steps state
  rootSteps: HierarchicalStep[];
  totalRootCount: number;
  isLoadingRoot: boolean;
  rootOffset: number;

  // Timeline bounds (calculated from root steps)
  minTimestamp: number;
  maxTimestamp: number;
  totalDuration: number;

  // Expanded state
  expandedStepIds: Set<string>;
  loadingChildrenIds: Set<string>;

  // Child steps cache (parentScopeId -> children[])
  childrenCache: Map<string, HierarchicalStep[]>;
  childrenTotalCounts: Map<string, number>;

  // Instance tracking (to reset when workflow or instance changes)
  currentWorkflowId: string | null;
  currentInstanceId: string | null;

  // Actions
  setRootSteps: (
    steps: HierarchicalStep[],
    totalCount: number,
    minTimestamp: number,
    maxTimestamp: number
  ) => void;
  appendRootSteps: (steps: HierarchicalStep[]) => void;
  setLoadingRoot: (loading: boolean) => void;
  incrementRootOffset: (pageSize: number) => void;

  toggleExpanded: (stepId: string) => void;
  setExpanded: (stepId: string, expanded: boolean) => void;
  setChildren: (
    parentScopeId: string,
    children: HierarchicalStep[],
    totalCount: number
  ) => void;
  appendChildren: (parentScopeId: string, children: HierarchicalStep[]) => void;
  setLoadingChildren: (stepId: string, loading: boolean) => void;

  setCurrentInstance: (
    workflowId: string | null,
    instanceId: string | null
  ) => void;
  reset: () => void;
}

export const useTimelineStore = create<TimelineState>()(
  devtools(
    immer((set) => ({
      // Initial state
      rootSteps: [],
      totalRootCount: 0,
      isLoadingRoot: false,
      rootOffset: 0,
      minTimestamp: 0,
      maxTimestamp: 0,
      totalDuration: 0,
      expandedStepIds: new Set(),
      loadingChildrenIds: new Set(),
      childrenCache: new Map(),
      childrenTotalCounts: new Map(),
      currentWorkflowId: null,
      currentInstanceId: null,

      // Actions
      setRootSteps: (steps, totalCount, minTimestamp, maxTimestamp) => {
        set((state) => {
          state.rootSteps = steps;
          state.totalRootCount = totalCount;
          state.minTimestamp = minTimestamp;
          state.maxTimestamp = maxTimestamp;
          state.totalDuration = maxTimestamp - minTimestamp;
          state.rootOffset = steps.length;
        });
      },

      appendRootSteps: (steps) => {
        set((state) => {
          state.rootSteps = [...state.rootSteps, ...steps];
          state.rootOffset = state.rootSteps.length;

          // Update max timestamp if new steps extend the timeline
          for (const step of steps) {
            const endTime = step.absoluteStartMs + (step.durationMs || 0);
            if (endTime > state.maxTimestamp) {
              state.maxTimestamp = endTime;
              state.totalDuration = state.maxTimestamp - state.minTimestamp;
            }
          }
        });
      },

      setLoadingRoot: (loading) => {
        set((state) => {
          state.isLoadingRoot = loading;
        });
      },

      incrementRootOffset: (pageSize) => {
        set((state) => {
          state.rootOffset += pageSize;
        });
      },

      toggleExpanded: (stepId) => {
        set((state) => {
          if (state.expandedStepIds.has(stepId)) {
            state.expandedStepIds.delete(stepId);
          } else {
            state.expandedStepIds.add(stepId);
          }
        });
      },

      setExpanded: (stepId, expanded) => {
        set((state) => {
          if (expanded) {
            state.expandedStepIds.add(stepId);
          } else {
            state.expandedStepIds.delete(stepId);
          }
        });
      },

      setChildren: (parentScopeId, children, totalCount) => {
        set((state) => {
          state.childrenCache.set(parentScopeId, children);
          state.childrenTotalCounts.set(parentScopeId, totalCount);
        });
      },

      appendChildren: (parentScopeId, children) => {
        set((state) => {
          const existing = state.childrenCache.get(parentScopeId) || [];
          state.childrenCache.set(parentScopeId, [...existing, ...children]);
        });
      },

      setLoadingChildren: (stepId, loading) => {
        set((state) => {
          if (loading) {
            state.loadingChildrenIds.add(stepId);
          } else {
            state.loadingChildrenIds.delete(stepId);
          }
        });
      },

      setCurrentInstance: (workflowId, instanceId) => {
        set((state) => {
          // Reset everything when workflow or instance changes
          if (
            state.currentWorkflowId !== workflowId ||
            state.currentInstanceId !== instanceId
          ) {
            state.currentWorkflowId = workflowId;
            state.currentInstanceId = instanceId;
            state.rootSteps = [];
            state.totalRootCount = 0;
            state.isLoadingRoot = false;
            state.rootOffset = 0;
            state.minTimestamp = 0;
            state.maxTimestamp = 0;
            state.totalDuration = 0;
            state.expandedStepIds = new Set();
            state.loadingChildrenIds = new Set();
            state.childrenCache = new Map();
            state.childrenTotalCounts = new Map();
          }
        });
      },

      reset: () => {
        set((state) => {
          state.rootSteps = [];
          state.totalRootCount = 0;
          state.isLoadingRoot = false;
          state.rootOffset = 0;
          state.minTimestamp = 0;
          state.maxTimestamp = 0;
          state.totalDuration = 0;
          state.expandedStepIds = new Set();
          state.loadingChildrenIds = new Set();
          state.childrenCache = new Map();
          state.childrenTotalCounts = new Map();
          state.currentWorkflowId = null;
          state.currentInstanceId = null;
        });
      },
    })),
    { name: 'timeline-store' }
  )
);
