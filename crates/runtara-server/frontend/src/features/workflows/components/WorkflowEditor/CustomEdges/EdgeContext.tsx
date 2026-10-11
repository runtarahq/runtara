import { createContext, useContext, ReactNode } from 'react';
import { Edge } from '@xyflow/react';
import type { ConditionSuggestion } from '@/shared/components/ui/condition-editor';
import type { OrthogonalRoute } from '../CustomNodes/layout';

interface EdgeInsertData {
  id: string;
  source: string;
  target: string;
  sourceHandle: string;
  position: { x: number; y: number };
}

/** Route editing offered on a selected edge: condition, priority, delete. */
export interface EdgeRouteActions {
  onUpdateRouteData: (
    edgeId: string,
    updates: { condition?: unknown; priority?: number }
  ) => void;
  onDeleteRoute: (edgeId: string) => void;
  conditionSuggestionsForEdge: (edge: Edge) => ConditionSuggestion[];
  stepName: (stepId: string) => string;
}

interface EdgeContextValue {
  onInsertClick?: (edgeData: EdgeInsertData) => void;
  /**
   * Adds a step in parallel with this edge: source->new->target alongside the
   * kept source->target, the re-converging shape unconditional fan-out needs.
   */
  onParallelClick?: (edgeData: EdgeInsertData) => void;
  routeActions?: EdgeRouteActions;
  allEdges: Edge[];
  edgeRoutes: Record<string, OrthogonalRoute>;
}

const EdgeContext = createContext<EdgeContextValue>({
  onInsertClick: undefined,
  allEdges: [],
  edgeRoutes: {},
});

export function EdgeContextProvider({
  children,
  onInsertClick,
  onParallelClick,
  routeActions,
  allEdges,
  edgeRoutes = {},
}: {
  children: ReactNode;
  onInsertClick?: (edgeData: EdgeInsertData) => void;
  onParallelClick?: (edgeData: EdgeInsertData) => void;
  routeActions?: EdgeRouteActions;
  allEdges: Edge[];
  edgeRoutes?: Record<string, OrthogonalRoute>;
}) {
  return (
    <EdgeContext.Provider
      value={{
        onInsertClick,
        onParallelClick,
        routeActions,
        allEdges,
        edgeRoutes,
      }}
    >
      {children}
    </EdgeContext.Provider>
  );
}

export function useEdgeContext() {
  return useContext(EdgeContext);
}
