import { NodeResizer, type NodeProps, useReactFlow } from '@xyflow/react';
import { forwardRef, useCallback } from 'react';

import { BaseNode } from '../BaseNode';
import { SNAP_GRID_SIZE } from '@/features/scenarios/config/workflow-editor';

const snapToGrid = (value: number) =>
  Math.round(value / SNAP_GRID_SIZE) * SNAP_GRID_SIZE;

interface BaseResizableNodeProps extends Partial<NodeProps> {
  id: string;
  selected?: boolean;
  name?: string;
  hasValidationError?: boolean;
  children?: React.ReactNode;
}

export const BaseResizableNode = forwardRef<
  HTMLDivElement,
  BaseResizableNodeProps
>(({ children, ...props }, ref) => {
  const { setNodes } = useReactFlow();

  // Track which handle is being resized
  const onResizeStart = useCallback(
    (_event: any, params: any) => {
      const direction = params.direction || [0, 0];
      const [x, y] = direction;
      const handle = {
        isLeft: x < 0,
        isRight: x > 0,
        isTop: y < 0,
        isBottom: y > 0,
      };

      // Store handle direction in node data for workflowStore to access
      setNodes((nodes) =>
        nodes.map((node) =>
          node.id === props.id
            ? { ...node, data: { ...node.data, __resizeHandle: handle } }
            : node
        )
      );
    },
    [props.id, setNodes]
  );

  const handleResize = useCallback(
    (_event: any, params: { width: number; height: number }) => {
      const snappedWidth = snapToGrid(params.width);
      const snappedHeight = snapToGrid(params.height);
      return { width: snappedWidth, height: snappedHeight };
    },
    []
  );

  const onResizeEnd = useCallback(() => {
    // Clean up handle tracking
    setNodes((nodes) =>
      nodes.map((node) => {
        if (node.id === props.id && node.data?.__resizeHandle) {
          const { __resizeHandle: _unused, ...restData } = node.data;
          void _unused;
          return { ...node, data: restData };
        }
        return node;
      })
    );
  }, [props.id, setNodes]);

  return (
    <BaseNode ref={ref} {...props}>
      <NodeResizer
        onResizeStart={onResizeStart}
        onResize={handleResize}
        onResizeEnd={onResizeEnd}
        minWidth={SNAP_GRID_SIZE * 4}
        minHeight={SNAP_GRID_SIZE * 4}
      />
      {children}
    </BaseNode>
  );
});

BaseResizableNode.displayName = 'BaseResizableNode';
