import { createContext, useContext } from 'react';
import type { CanvasDetailLevel } from './CustomNodes/node-size';

interface CanvasDisplayContextValue {
  /** Whether step cards list their configured inputs. */
  detailLevel: CanvasDetailLevel;
  /** Whether hovering a step shows its full configuration. */
  stepPreviews: boolean;
  /** Whether the preview may offer "double-click to edit". */
  editable: boolean;
  /** Step id → name, so references read as the step they come from. */
  stepNames: ReadonlyMap<string, string>;
  /** Step id → step type, to tell a Split's item scope from the root. */
  stepTypes: ReadonlyMap<string, string>;
}

// Outside a provider (BaseNode rendered on its own, as in unit tests) cards
// are compact with no previews.
const CanvasDisplayContext = createContext<CanvasDisplayContextValue>({
  detailLevel: 'compact',
  stepPreviews: false,
  editable: false,
  stepNames: new Map(),
  stepTypes: new Map(),
});

export const CanvasDisplayProvider = CanvasDisplayContext.Provider;

export function useCanvasDisplay() {
  return useContext(CanvasDisplayContext);
}
