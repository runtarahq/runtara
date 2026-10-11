/**
 * Per-viewer editor preferences (which view, how much detail on the canvas).
 *
 * Browser storage is a convenience here: it can be unavailable (private
 * windows, blocked site data) or hold a stale value, and the editor must
 * behave the same with the defaults.
 */
import type { CanvasDetailLevel } from './CustomNodes/node-size';

export type WorkflowEditorView = 'canvas' | 'timeline';

const VIEW_KEY = 'runtara.workflowEditor.view';
const DETAIL_KEY = 'runtara.workflowEditor.canvasDetail';

export const DEFAULT_EDITOR_VIEW: WorkflowEditorView = 'canvas';
export const DEFAULT_CANVAS_DETAIL: CanvasDetailLevel = 'detailed';

function read(key: string): string | null {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string) {
  try {
    window.localStorage.setItem(key, value);
  } catch {
    // Not persisting a preference is harmless.
  }
}

export function readEditorView(): WorkflowEditorView {
  const value = read(VIEW_KEY);
  return value === 'canvas' || value === 'timeline'
    ? value
    : DEFAULT_EDITOR_VIEW;
}

export function writeEditorView(view: WorkflowEditorView) {
  write(VIEW_KEY, view);
}

export function readCanvasDetail(): CanvasDetailLevel {
  const value = read(DETAIL_KEY);
  return value === 'compact' || value === 'detailed'
    ? value
    : DEFAULT_CANVAS_DETAIL;
}

export function writeCanvasDetail(detail: CanvasDetailLevel) {
  write(DETAIL_KEY, detail);
}
