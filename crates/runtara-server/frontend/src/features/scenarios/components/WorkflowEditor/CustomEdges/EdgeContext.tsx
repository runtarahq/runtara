import { createContext, useContext, ReactNode } from 'react';
import { Edge } from '@xyflow/react';

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
}

const EdgeContext = createContext<EdgeContextValue>({
  onInsertClick: undefined,
  allEdges: [],
});

export function EdgeContextProvider({
  children,
  onInsertClick,
  allEdges,
}: {
  children: ReactNode;
  onInsertClick?: (edgeData: EdgeInsertData) => void;
  allEdges: Edge[];
}) {
  return (
    <EdgeContext.Provider value={{ onInsertClick, allEdges }}>
      {children}
    </EdgeContext.Provider>
  );
}

export function useEdgeContext() {
  return useContext(EdgeContext);
}
