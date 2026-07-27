import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from 'react';
import { Button } from '@/shared/components/ui/button';
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/shared/components/ui/alert-dialog';
import { Icons } from '@/shared/components/icons';
import { cn } from '@/lib/utils';
import { NodeForm } from '../NodeForm';
import { NodeFormProvider } from '../NodeForm/NodeFormProvider';
import * as form from '../NodeForm/NodeFormItem';
import { SchemaField } from '../EditorSidebar/SchemaFieldsEditor';
import {
  connectorGeometry,
  isDocked,
  NODE_CONFIG_PANEL_WIDTH,
  panelGeometry,
  type Rect,
} from './dock-position';

/** Simple variable type matching the WorkflowEditor prop type */
interface SimpleVariable {
  name: string;
  value: unknown;
  type: string;
  description?: string | null;
}

interface NodeConfigPanelProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  nodeId: string;
  nodeData: form.SchemaType;
  originalNodeData: form.SchemaType;
  outputSchemaFields?: SchemaField[];
  /** Workflow input schema fields for variable suggestions */
  inputSchemaFields?: SchemaField[];
  /** Workflow variables (constants) for variable suggestions */
  variables?: SimpleVariable[];
  onSave: (
    nodeId: string,
    data: form.SchemaType
  ) => void | boolean | Promise<void | boolean>;
  onStagedChange?: (nodeId: string, data: form.SchemaType) => void;
  onReset?: (nodeId: string) => void;
  onDelete?: (nodeId: string) => void;
  /** When true, the panel is for creating a new node (no delete, different title) */
  isCreate?: boolean;
  /** Parent node ID for computing previous steps (used when creating new nodes) */
  parentNodeId?: string;
  /** Create flows: the real enclosing container (null = top level). */
  createContainerId?: string | null;
}

/** Live rect of the canvas node this panel is anchored to, if it is on screen. */
function useAnchorRect(nodeId: string, enabled: boolean): Rect | null {
  const [rect, setRect] = useState<Rect | null>(null);

  useLayoutEffect(() => {
    if (!enabled) {
      setRect(null);
      return;
    }

    let frame = 0;
    const measure = () => {
      // Anchor to whichever representation of the step is on screen. React
      // Flow stamps data-id on each canvas node (a DOM read picks up pan and
      // zoom for free rather than re-deriving the viewport transform), and the
      // timeline stamps data-timeline-node-id on each row. One panel serves
      // both views.
      const id = CSS.escape(nodeId);
      const el =
        document.querySelector<HTMLElement>(
          `.react-flow__node[data-id="${id}"]`
        ) ??
        // The card, not the row wrapper: the wrapper also contains the
        // add-branch/add-error controls, which drags its centre below the
        // visible tile.
        document.querySelector<HTMLElement>(
          `[data-timeline-node-card="${id}"]`
        ) ??
        document.querySelector<HTMLElement>(`[data-timeline-node-id="${id}"]`);
      if (!el) {
        setRect(null);
        return;
      }
      const r = el.getBoundingClientRect();
      setRect((prev) =>
        prev &&
        prev.top === r.top &&
        prev.bottom === r.bottom &&
        prev.left === r.left &&
        prev.right === r.right
          ? prev
          : { top: r.top, bottom: r.bottom, left: r.left, right: r.right }
      );
    };

    // The canvas pans and zooms without firing scroll/resize, so poll on a
    // frame loop while the panel is open. Cheap: one getBoundingClientRect.
    const loop = () => {
      measure();
      frame = requestAnimationFrame(loop);
    };
    loop();
    return () => cancelAnimationFrame(frame);
  }, [nodeId, enabled]);

  return rect;
}

export function NodeConfigPanel({
  open,
  onOpenChange,
  nodeId,
  nodeData,
  originalNodeData,
  outputSchemaFields,
  inputSchemaFields,
  variables,
  onSave,
  onStagedChange,
  // onReset is kept in the props interface for backwards compatibility but not
  // used (we reset locally)
  onDelete,
  isCreate = false,
  parentNodeId,
  createContainerId,
}: NodeConfigPanelProps) {
  const stagedDataRef = useRef<form.SchemaType>(nodeData);
  const prevOpenRef = useRef(false);
  const formContainerRef = useRef<HTMLDivElement | null>(null);
  const panelRef = useRef<HTMLElement | null>(null);

  const [isDirty, setIsDirty] = useState(false);
  const [confirmDiscardOpen, setConfirmDiscardOpen] = useState(false);
  const [viewport, setViewport] = useState(() => ({
    width: typeof window === 'undefined' ? 1600 : window.innerWidth,
    height: typeof window === 'undefined' ? 900 : window.innerHeight,
  }));
  // The area the panel may occupy: the editor, not the window. Keeps a margin
  // at the top and clears the bottom bar rather than sitting on top of it.
  const [bounds, setBounds] = useState<Rect | null>(null);

  useEffect(() => {
    const measure = () => {
      setViewport({ width: window.innerWidth, height: window.innerHeight });
      const host = document.querySelector<HTMLElement>(
        '[data-workflow-editor-root]'
      );
      if (!host) {
        setBounds(null);
        return;
      }
      const r = host.getBoundingClientRect();
      setBounds({ top: r.top, bottom: r.bottom, left: r.left, right: r.right });
    };
    measure();
    window.addEventListener('resize', measure);
    return () => window.removeEventListener('resize', measure);
  }, [open]);

  useEffect(() => {
    if (open && !prevOpenRef.current) {
      stagedDataRef.current = nodeData;
    }
    prevOpenRef.current = open;
  }, [open, nodeData]);

  const docked = isDocked(viewport);
  // A pending node has no canvas node, but the timeline renders a placeholder
  // card for it under the same id — so the lookup is worth attempting either
  // way, and simply finds nothing on the canvas.
  const anchorRect = useAnchorRect(nodeId, open && docked);
  const geo = panelGeometry(
    bounds ?? {
      top: 0,
      bottom: viewport.height,
      left: 0,
      right: viewport.width,
    },
    NODE_CONFIG_PANEL_WIDTH
  );
  const panelRect: Rect = {
    top: geo.top,
    bottom: geo.top + (panelRef.current?.offsetHeight ?? geo.maxHeight),
    left: geo.left,
    right: geo.left + geo.width,
  };
  const connector = connectorGeometry(anchorRect, panelRect);

  const handleSubmit = useCallback(
    async (data: form.SchemaType) => {
      const saved = await onSave(nodeId, data);
      if (saved !== false) {
        onOpenChange(false);
      }
    },
    [nodeId, onSave, onOpenChange]
  );

  const handleChangeRef = useRef<(data: form.SchemaType) => void>();
  handleChangeRef.current = (data: form.SchemaType) => {
    stagedDataRef.current = data;
    if (isCreate) {
      onStagedChange?.(nodeId, data);
    }
  };
  const handleChange = useCallback((data: form.SchemaType) => {
    handleChangeRef.current?.(data);
  }, []);

  const handleSave = useCallback(() => {
    const formElement = formContainerRef.current?.querySelector('form');
    if (!formElement) {
      console.error('NodeConfigPanel: form element was not found');
      return;
    }
    formElement.requestSubmit();
  }, []);

  const discard = useCallback(() => {
    stagedDataRef.current = originalNodeData;
    setIsDirty(false);
    setConfirmDiscardOpen(false);
    onOpenChange(false);
  }, [originalNodeData, onOpenChange]);

  const requestClose = useCallback(() => {
    if (isDirty) {
      setConfirmDiscardOpen(true);
      return;
    }
    discard();
  }, [isDirty, discard]);

  const handleDelete = useCallback(() => {
    onDelete?.(nodeId);
    onOpenChange(false);
  }, [nodeId, onDelete, onOpenChange]);

  // The panel is modeless by design — you can click another node to re-anchor
  // it — so Escape is wired here rather than inherited from a dialog.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      // Let a nested popover/dialog take the key first.
      if (document.querySelector('[role="alertdialog"], [role="dialog"]'))
        return;
      e.stopPropagation();
      requestClose();
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [open, requestClose]);

  if (!open) return null;

  const stepName = nodeData?.name || 'Step';
  const stepType = nodeData?.stepType || '';
  const title = isCreate ? `New ${stepName}` : stepName;
  const description = isCreate
    ? `Configure the new ${stepType || 'step'} before adding it to the workflow`
    : stepType
      ? `${stepType} step configuration`
      : '';

  return (
    <>
      {connector.visible && (
        <div
          aria-hidden="true"
          className="pointer-events-none fixed z-40 h-px bg-primary/40"
          style={{
            top: connector.top,
            left: connector.left,
            width: connector.width,
          }}
        >
          <span className="absolute -top-[3px] right-[-1px] size-[7px] rounded-full bg-primary" />
        </div>
      )}

      <aside
        ref={panelRef}
        aria-label={description || 'Step configuration'}
        data-testid="node-config-dialog"
        data-node-id={nodeId}
        data-step-type={stepType}
        className={cn(
          'fixed z-50 flex flex-col rounded-md border bg-card text-card-foreground shadow-lg',
          // Below the dock breakpoint there is no gutter to sit in, so the
          // panel becomes a bottom sheet.
          !docked && 'inset-x-3 bottom-0 max-h-[72vh] rounded-b-none'
        )}
        style={
          docked
            ? {
                top: geo.top,
                left: geo.left,
                width: geo.width,
                maxHeight: geo.maxHeight,
              }
            : undefined
        }
      >
        <div className="flex shrink-0 items-start justify-between gap-3 border-b px-4 py-3">
          <div className="min-w-0">
            <p className="truncate text-base font-semibold">{title}</p>
            {description && (
              <p className="text-xs text-muted-foreground">{description}</p>
            )}
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            onClick={requestClose}
            aria-label="Close step configuration"
          >
            <Icons.x aria-hidden="true" />
          </Button>
        </div>

        <div
          ref={formContainerRef}
          className="min-h-0 flex-1 overflow-y-auto overscroll-contain px-4 py-3"
        >
          <NodeFormProvider
            nodeId={nodeId}
            parentNodeId={parentNodeId}
            createContainerId={createContainerId}
            outputSchemaFields={outputSchemaFields}
            inputSchemaFields={inputSchemaFields}
            variables={variables}
          >
            <NodeForm
              key={nodeId}
              isEdit={!isCreate}
              values={nodeData}
              originalValues={originalNodeData}
              onChange={handleChange}
              onDirtyChange={setIsDirty}
              onSubmit={handleSubmit}
              onDelete={isCreate ? undefined : handleDelete}
              contentScrollable={false}
              hideActions
            />
          </NodeFormProvider>
        </div>

        <div className="flex shrink-0 items-center justify-end gap-2 border-t px-4 py-3">
          {!isCreate && onDelete && (
            <Button
              type="button"
              variant="destructiveGhost"
              className="mr-auto"
              onClick={handleDelete}
              data-testid="node-config-delete"
            >
              Delete
            </Button>
          )}
          <Button
            type="button"
            variant="outline"
            onClick={requestClose}
            data-testid="node-config-cancel"
          >
            Cancel
          </Button>
          <Button
            type="button"
            onClick={handleSave}
            data-testid="node-config-save"
          >
            {isCreate ? 'Add step' : 'Save'}
          </Button>
        </div>
      </aside>

      <AlertDialog
        open={confirmDiscardOpen}
        onOpenChange={setConfirmDiscardOpen}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Discard changes to this step?</AlertDialogTitle>
            <AlertDialogDescription>
              This step has edits you have not saved. Closing now throws them
              away &mdash; there is no undo.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Keep editing</AlertDialogCancel>
            <AlertDialogAction onClick={discard}>Discard</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
