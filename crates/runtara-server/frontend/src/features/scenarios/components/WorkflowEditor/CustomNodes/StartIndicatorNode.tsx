import { memo } from 'react';
import { NodeProps, Position, Handle } from '@xyflow/react';
import { Play, Plus } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';

/**
 * Virtual Start Indicator Node
 *
 * This is a non-interactive visual indicator that shows the entry point of the scenario.
 * It's not part of the actual execution graph - just a visual hint that the schema
 * is set at the scenario level and the first step is the entry point.
 *
 * When there are no steps, it shows a "+" button to add the first step.
 */
function StartIndicatorNodeComponent({ data }: NodeProps) {
  const hasEntryPoint =
    (data as { hasEntryPoint?: boolean }).hasEntryPoint !== false;
  const onAddFirstStep = (data as { onAddFirstStep?: () => void })
    .onAddFirstStep;

  return (
    <div className="flex items-center justify-center w-full h-full px-3 gap-1.5 rounded-full bg-muted/40">
      {/* Icon */}
      <div className="flex-shrink-0 flex items-center justify-center text-muted-foreground/50">
        <Play className="w-3 h-3 fill-current" />
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
          className="!bg-muted-foreground/40 !border-0 !rounded-full !w-2 !h-2"
          isConnectable={false}
        />
      )}

      {/* Add first step button when no entry point */}
      {!hasEntryPoint && (
        <div
          className="absolute flex items-center pointer-events-none"
          style={{
            right: '-32px',
            top: '50%',
            transform: 'translateY(-50%)',
          }}
        >
          <div className="bg-border h-[1px] w-4" />
          <Button
            className="w-5 h-5 rounded-full [&_svg]:size-3 shadow-md pointer-events-auto nodrag nopan"
            variant="outline"
            size="icon"
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
