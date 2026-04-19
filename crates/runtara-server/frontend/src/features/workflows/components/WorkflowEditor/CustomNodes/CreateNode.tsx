import { memo, useCallback, useState } from 'react';
import { v4 as uuidv4 } from 'uuid';
import { Node, NodeProps, useReactFlow } from '@xyflow/react';
import { BaseNodeForm } from '../NodeForm/BaseNodeForm.tsx';
import * as form from '@/features/workflows/components/WorkflowEditor/NodeForm/NodeFormItem.tsx';
import { Dialog, DialogContent } from '@/shared/components/ui/dialog.tsx';
import {
  NODE_TYPE_SIZES,
  NODE_TYPES,
  STEP_TYPES,
} from '@/features/workflows/config/workflow.ts';
import { BaseNode } from '../BaseNode.tsx';
import { useExecutionStore } from '@/features/workflows/stores/executionStore';

function CreateNodeComponent(props: NodeProps<Node>) {
  const { id } = props;

  const [openCreate, setOpenCreate] = useState<boolean>(false);

  const { updateNode } = useReactFlow();

  // Check if workflow is executing (read-only mode)
  const isExecuting = useExecutionStore((state) => !!state.executingInstanceId);

  const handleCreate = useCallback(
    (data: form.SchemaType) => {
      const newId = uuidv4();
      const type = STEP_TYPES[data.stepType] || NODE_TYPES.BasicNode;
      const style = NODE_TYPE_SIZES[type];

      const newNode: Node = {
        id: newId,
        type,
        data: {
          id: newId,
          ...data,
        },
        position: {
          x: 0,
          y: 0,
        },
        style,
      };

      updateNode(id, newNode);
      setOpenCreate(false);
    },
    [id, updateNode]
  );

  const handleOpen = () => {
    // Don't allow creating nodes during execution
    if (!isExecuting) {
      setOpenCreate(true);
    }
  };

  // Don't render the CreateNode during execution (hide the "+" button)
  if (isExecuting) {
    return null;
  }

  return (
    <>
      <BaseNode stepType="Create" onClick={handleOpen} />

      <Dialog open={openCreate} onOpenChange={setOpenCreate}>
        <DialogContent
          className="flex flex-col w-[50vw] h-[70vh] max-w-none overflow-hidden"
          hideCloseButton
        >
          <BaseNodeForm
            initValues={form.initialValues as form.SchemaType}
            onSubmit={handleCreate}
          />
        </DialogContent>
      </Dialog>
    </>
  );
}

export const CreateNode = memo(CreateNodeComponent);
