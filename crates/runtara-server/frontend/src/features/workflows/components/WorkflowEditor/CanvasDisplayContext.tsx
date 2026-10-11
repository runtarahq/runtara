import { createContext, useContext } from 'react';
import type { CanvasDetailLevel } from './CustomNodes/node-size';

interface CanvasDisplayContextValue {
  /** Whether step cards list their configured inputs. */
  detailLevel: CanvasDetailLevel;
  /** Whether hovering a step shows its full configuration. */
  stepPreviews: boolean;
  /** Whether the preview may offer "double-click to edit". */
  editable: boolean;
}

// The default is what any canvas outside the workflow editor gets (the replay
// graph reuses BaseNode): compact cards, no previews.
const CanvasDisplayContext = createContext<CanvasDisplayContextValue>({
  detailLevel: 'compact',
  stepPreviews: false,
  editable: false,
});

export const CanvasDisplayProvider = CanvasDisplayContext.Provider;

export function useCanvasDisplay() {
  return useContext(CanvasDisplayContext);
}
