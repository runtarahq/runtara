import { NodeForm } from './index.tsx';
import { NodeFormProvider } from './NodeFormProvider.tsx';
import { SchemaType } from './NodeFormItem.tsx';

type Props = {
  nodeId?: string;
  parentNodeId?: string;
  initValues: SchemaType;
  onSubmit: (data: SchemaType) => void;
};

export function BaseNodeForm({
  nodeId,
  parentNodeId,
  initValues,
  onSubmit,
}: Props) {
  const handleSubmit = (data: SchemaType) => {
    onSubmit(data);
  };

  return (
    <NodeFormProvider nodeId={nodeId} parentNodeId={parentNodeId}>
      <NodeForm isEdit={!!nodeId} values={initValues} onSubmit={handleSubmit} />
    </NodeFormProvider>
  );
}
