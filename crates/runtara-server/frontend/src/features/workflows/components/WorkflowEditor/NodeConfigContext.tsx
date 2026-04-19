import { createContext, useContext } from 'react';

interface NodeConfigContextValue {
  openNodeConfig: (nodeId: string) => void;
}

const NodeConfigContext = createContext<NodeConfigContextValue>({
  openNodeConfig: () => {},
});

export const NodeConfigProvider = NodeConfigContext.Provider;

export function useNodeConfigContext() {
  return useContext(NodeConfigContext);
}
