import { createContext, useContext } from 'react';
import type {
  InputIntentRequest,
  RetainedInput,
} from '../utils/retained-inputs';

type Controller = {
  inputs: RetainedInput[];
  submit: (request: InputIntentRequest, label: string) => Promise<boolean>;
  retry: (operationId: string) => Promise<boolean>;
};

export const ManagedInputContext = createContext<Controller | null>(null);

export function useManagedInputSubmissions() {
  const controller = useContext(ManagedInputContext);
  if (!controller)
    throw new Error('Managed inputs require a page submission owner');
  return controller;
}
