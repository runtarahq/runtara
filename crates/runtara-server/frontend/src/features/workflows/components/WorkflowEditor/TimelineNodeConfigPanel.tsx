import { useCallback, useRef } from 'react';
import { Check, X } from 'lucide-react';

import { Button } from '@/shared/components/ui/button';
import { NodeForm } from './NodeForm';
import { NodeFormProvider } from './NodeForm/NodeFormProvider';
import * as form from './NodeForm/NodeFormItem';
import type { SimpleVariable } from './NodeForm/NodeFormContext';
import type { SchemaField } from './EditorSidebar/SchemaFieldsEditor';

interface TimelineNodeConfigPanelProps {
  nodeId: string;
  parentNodeId?: string;
  nodeData: form.SchemaType;
  originalNodeData: form.SchemaType;
  outputSchemaFields?: SchemaField[];
  inputSchemaFields?: SchemaField[];
  variables?: SimpleVariable[];
  onSave: (nodeId: string, data: form.SchemaType) => void;
  onReset?: (nodeId: string) => void;
  onDelete?: (nodeId: string) => void;
  onCancel: () => void;
  isCreate?: boolean;
}

export function TimelineNodeConfigPanel({
  nodeId,
  parentNodeId,
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

  const handleSubmit = useCallback(
    (data: form.SchemaType) => {
      onSave(nodeId, data);
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
    <div className="bg-card">
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
          onClick={onCancel}
          aria-label="Close inline editor"
        >
          <X aria-hidden="true" />
        </Button>
      </div>

      <div ref={formContainerRef} className="px-4 py-3">
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
            onSubmit={handleSubmit}
            onReset={isCreate ? undefined : handleReset}
            onDelete={isCreate ? undefined : handleDelete}
            contentScrollable={false}
            hideActions={isCreate}
          />
        </NodeFormProvider>
      </div>

      <div className="flex justify-end gap-2 border-t px-4 py-3">
        <Button type="button" variant="outline" onClick={onCancel}>
          <X aria-hidden="true" />
          Cancel
        </Button>
        <Button type="button" onClick={handleSave}>
          <Check aria-hidden="true" />
          Save
        </Button>
      </div>
    </div>
  );
}
