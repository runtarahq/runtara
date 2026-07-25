import { useCallback, useRef, useState } from 'react';
import { Check, X } from 'lucide-react';

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
import { NodeForm } from './NodeForm';
import { NodeFormProvider } from './NodeForm/NodeFormProvider';
import * as form from './NodeForm/NodeFormItem';
import type { SimpleVariable } from './NodeForm/NodeFormContext';
import type { SchemaField } from './EditorSidebar/SchemaFieldsEditor';

interface TimelineNodeConfigPanelProps {
  nodeId: string;
  parentNodeId?: string;
  /** Create flows: the real enclosing container (null = top level). */
  createContainerId?: string | null;
  nodeData: form.SchemaType;
  originalNodeData: form.SchemaType;
  outputSchemaFields?: SchemaField[];
  inputSchemaFields?: SchemaField[];
  variables?: SimpleVariable[];
  onSave: (
    nodeId: string,
    data: form.SchemaType
  ) => void | boolean | Promise<void | boolean>;
  onReset?: (nodeId: string) => void;
  onDelete?: (nodeId: string) => void;
  onCancel: () => void;
  isCreate?: boolean;
}

export function TimelineNodeConfigPanel({
  nodeId,
  parentNodeId,
  createContainerId,
  nodeData,
  originalNodeData,
  outputSchemaFields,
  inputSchemaFields,
  variables,
  onSave,
  onReset,
  onDelete,
  onCancel,
  isCreate = false,
}: TimelineNodeConfigPanelProps) {
  const formContainerRef = useRef<HTMLDivElement | null>(null);

  // Unsaved-edit guard for the inline panel. Closing here is one click and the
  // panel remounts (dropping form state) whenever another row is edited.
  const [isDirty, setIsDirty] = useState(false);
  const [confirmDiscardOpen, setConfirmDiscardOpen] = useState(false);

  const requestClose = useCallback(() => {
    if (isDirty) {
      setConfirmDiscardOpen(true);
      return;
    }
    onCancel();
  }, [isDirty, onCancel]);

  const discard = useCallback(() => {
    setIsDirty(false);
    setConfirmDiscardOpen(false);
    onCancel();
  }, [onCancel]);

  const handleSubmit = useCallback(
    async (data: form.SchemaType) => {
      await onSave(nodeId, data);
    },
    [nodeId, onSave]
  );

  const handleSave = useCallback(() => {
    const formElement = formContainerRef.current?.querySelector('form');
    if (!formElement) {
      console.error('TimelineNodeConfigPanel: form element was not found');
      return;
    }

    formElement.requestSubmit();
  }, []);

  const handleReset = useCallback(() => {
    onReset?.(nodeId);
  }, [nodeId, onReset]);

  const handleDelete = useCallback(() => {
    onDelete?.(nodeId);
  }, [nodeId, onDelete]);

  const stepType = nodeData?.stepType || 'step';

  return (
    <div
      className="bg-card"
      data-testid="timeline-node-config-panel"
      data-node-id={nodeId}
      data-step-type={stepType}
    >
      <div className="flex flex-wrap items-center justify-between gap-3 border-b bg-muted/30 px-4 py-2">
        <div className="min-w-0">
          <p className="text-xs text-muted-foreground">
            {stepType} step configuration
          </p>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          onClick={requestClose}
          aria-label="Close inline editor"
        >
          <X aria-hidden="true" />
        </Button>
      </div>

      <div ref={formContainerRef} className="px-4 py-3">
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
            onDirtyChange={setIsDirty}
            onSubmit={handleSubmit}
            onReset={isCreate ? undefined : handleReset}
            onDelete={isCreate ? undefined : handleDelete}
            contentScrollable={false}
            hideActions={isCreate}
          />
        </NodeFormProvider>
      </div>

      <div className="flex justify-end gap-2 border-t px-4 py-3">
        <Button
          type="button"
          variant="outline"
          onClick={requestClose}
          data-testid="timeline-node-config-cancel"
        >
          <X aria-hidden="true" />
          Cancel
        </Button>
        <Button
          type="button"
          onClick={handleSave}
          data-testid="timeline-node-config-save"
        >
          <Check aria-hidden="true" />
          Save
        </Button>
      </div>

      <AlertDialog
        open={confirmDiscardOpen}
        onOpenChange={setConfirmDiscardOpen}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Discard changes to this step?</AlertDialogTitle>
            <AlertDialogDescription>
              This step has edits you have not saved. Closing now throws them
              away — there is no undo.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Keep editing</AlertDialogCancel>
            <AlertDialogAction onClick={discard}>Discard</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
