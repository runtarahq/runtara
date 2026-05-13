import { useCallback, useEffect, useRef } from 'react';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from '@/shared/components/ui/dialog';
import { Button } from '@/shared/components/ui/button';
import { NodeForm } from '../NodeForm';
import { NodeFormProvider } from '../NodeForm/NodeFormProvider';
import * as form from '../NodeForm/NodeFormItem';
import { SchemaField } from '../EditorSidebar/SchemaFieldsEditor';

/** Simple variable type matching the WorkflowEditor prop type */
interface SimpleVariable {
  name: string;
  value: string;
  type: string;
  description?: string | null;
}

interface NodeConfigDialogProps {
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
  /** When true, dialog is for creating a new node (no delete button, different title) */
  isCreate?: boolean;
  /** Parent node ID for computing previous steps (used when creating new nodes) */
  parentNodeId?: string;
}

export function NodeConfigDialog({
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
  // onReset is kept in props interface for backwards compatibility but not used (we reset locally)
  onDelete,
  isCreate = false,
  parentNodeId,
}: NodeConfigDialogProps) {
  // Store latest form data via ref to avoid circular updates
  // We use a ref instead of state because we don't want changes to trigger re-renders
  // that would reset the form values prop
  const stagedDataRef = useRef<form.SchemaType>(nodeData);

  // Track previous open state using a ref to avoid re-render loops
  const prevOpenRef = useRef(false);
  const formContainerRef = useRef<HTMLDivElement | null>(null);

  // Reset staged data ref when dialog opens
  useEffect(() => {
    if (open && !prevOpenRef.current) {
      // Dialog just opened - reset to current node values
      stagedDataRef.current = nodeData;
    }
    prevOpenRef.current = open;
  }, [open, nodeData]);

  const handleSubmit = useCallback(
    async (data: form.SchemaType) => {
      const saved = await onSave(nodeId, data);
      if (saved !== false) {
        onOpenChange(false);
      }
    },
    [nodeId, onSave, onOpenChange]
  );

  // Use a ref to store the change handler so it's always stable
  const handleChangeRef = useRef<(data: form.SchemaType) => void>();

  // Update the ref with the latest logic on each render
  handleChangeRef.current = (data: form.SchemaType) => {
    // Always keep the latest form values in the local ref so Save persists edits
    stagedDataRef.current = data;

    if (isCreate) {
      // In create mode, continue to call parent's onStagedChange for backwards compatibility
      onStagedChange?.(nodeId, data);
    }
  };

  // Create a stable callback that calls through the ref
  const handleChange = useCallback((data: form.SchemaType) => {
    handleChangeRef.current?.(data);
  }, []);

  const handleSave = useCallback(() => {
    const formElement = formContainerRef.current?.querySelector('form');
    if (!formElement) {
      console.error('NodeConfigDialog: form element was not found');
      return;
    }

    formElement.requestSubmit();
  }, []);

  const handleCancel = useCallback(() => {
    stagedDataRef.current = originalNodeData;
    onOpenChange(false);
  }, [originalNodeData, onOpenChange]);

  const handleReset = useCallback(() => {
    // Don't update ref here - the form will handle the reset and call onChange
    // which will update the ref
  }, []);

  const handleDelete = useCallback(() => {
    onDelete?.(nodeId);
    onOpenChange(false);
  }, [nodeId, onDelete, onOpenChange]);

  // Get step name for dialog title
  const stepName = nodeData?.name || 'Step';
  const stepType = nodeData?.stepType || '';

  const dialogTitle = isCreate ? `New ${stepName}` : stepName;
  const dialogDescription = isCreate
    ? `Configure the new ${stepType || 'step'} before adding it to the workflow`
    : stepType
      ? `${stepType} step configuration`
      : '';

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className="max-w-4xl max-h-[90vh] flex flex-col p-0 gap-0"
        data-testid="node-config-dialog"
        data-node-id={nodeId}
        data-step-type={stepType}
      >
        <DialogHeader className="px-6 py-4 border-b shrink-0">
          <DialogTitle className="text-lg font-semibold">
            {dialogTitle}
          </DialogTitle>
          <DialogDescription className="text-sm text-muted-foreground">
            {dialogDescription}
          </DialogDescription>
        </DialogHeader>

        <div
          ref={formContainerRef}
          className="flex-1 overflow-y-auto px-6 py-4 min-h-0"
        >
          <NodeFormProvider
            nodeId={nodeId}
            parentNodeId={parentNodeId}
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
              onSubmit={handleSubmit}
              onReset={isCreate ? undefined : handleReset}
              onDelete={isCreate ? undefined : handleDelete}
            />
          </NodeFormProvider>
        </div>

        {!isCreate && (
          <DialogFooter className="px-6 py-4 border-t shrink-0">
            <Button variant="outline" onClick={handleCancel}>
              Cancel
            </Button>
            <Button onClick={handleSave}>Save</Button>
          </DialogFooter>
        )}
      </DialogContent>
    </Dialog>
  );
}
