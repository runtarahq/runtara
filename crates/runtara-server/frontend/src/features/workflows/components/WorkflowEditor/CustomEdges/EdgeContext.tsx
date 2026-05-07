import { createContext, useContext, ReactNode } from 'react';
import { Edge } from '@xyflow/react';
import type { OrthogonalRoute } from '../CustomNodes/layout';

interface EdgeInsertData {
  id: string;
  source: string;
  target: string;
  sourceHandle: string;
  position: { x: number; y: number };
}

interface EdgeContextValue {
  onInsertClick?: (edgeData: EdgeInsertData) => void;
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
  allEdges,
  edgeRoutes = {},
}: {
  children: ReactNode;
  onInsertClick?: (edgeData: EdgeInsertData) => void;
  allEdges: Edge[];
  edgeRoutes?: Record<string, OrthogonalRoute>;
}) {
  return (
    <EdgeContext.Provider value={{ onInsertClick, allEdges, edgeRoutes }}>
      {children}
    </EdgeContext.Provider>
  );
}

export function useEdgeContext() {
  return useContext(EdgeContext);
}
