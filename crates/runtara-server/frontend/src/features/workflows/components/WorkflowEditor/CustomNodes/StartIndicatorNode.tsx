import { memo } from 'react';
import { NodeProps, Position, Handle } from '@xyflow/react';
import { Play, Plus } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import {
  NODE_TYPE_SIZES,
  NODE_TYPES,
} from '@/features/workflows/config/workflow.ts';

/**
 * Virtual Start Indicator Node
 *
 * This is a non-interactive visual indicator that shows the entry point of the workflow.
 * It's not part of the actual execution graph - just a visual hint that the schema
 * is set at the workflow level and the first step is the entry point.
 *
 * When there are no steps, it shows a "+" button to add the first step.
 */
function StartIndicatorNodeComponent({ data }: NodeProps) {
  const hasEntryPoint =
    (data as { hasEntryPoint?: boolean }).hasEntryPoint !== false;
  const onAddFirstStep = (data as { onAddFirstStep?: () => void })
    .onAddFirstStep;

  return (
    <div
      className="flex items-center justify-center gap-1.5 rounded-full bg-muted/40 px-3"
      style={{
        width: NODE_TYPE_SIZES[NODE_TYPES.StartIndicatorNode].width,
        height: NODE_TYPE_SIZES[NODE_TYPES.StartIndicatorNode].height,
      }}
    >
      {/* Icon */}
      <div className="flex flex-shrink-0 items-center justify-center text-muted-foreground/50">
        <Play className="size-3 fill-current" />
      </div>

      {/* Label */}
      <span className="text-xs font-medium text-muted-foreground/50">
        Start
      </span>

      {/* Source handle to connect to the first step - pill shape matching other nodes */}
      {hasEntryPoint && (
        <Handle
          type="source"
          position={Position.Right}
          id="source"
          className="!h-2 !w-2 !rounded-full !border-0 !bg-muted-foreground/40"
          isConnectable={false}
        />
      )}

      {/* Add first step button when no entry point */}
      {!hasEntryPoint && (
        <div className="pointer-events-none absolute -right-8 top-1/2 flex -translate-y-1/2 items-center">
          <div className="h-px w-4 bg-border" />
          <Button
            className="nodrag nopan pointer-events-auto size-5 rounded-full shadow-md [&_svg]:size-3"
            variant="outline"
            size="icon"
            aria-label="Add first workflow step"
            onClick={(e) => {
              e.stopPropagation();
              onAddFirstStep?.();
            }}
          >
            <Plus />
          </Button>
        </div>
      )}
    </div>
  );
}

export const StartIndicatorNode = memo(StartIndicatorNodeComponent);
