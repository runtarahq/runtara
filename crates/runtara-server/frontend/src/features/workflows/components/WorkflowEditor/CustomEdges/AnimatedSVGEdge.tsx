import {
  BaseEdge,
  EdgeLabelRenderer,
  type EdgeProps,
  getBezierPath,
} from '@xyflow/react';
import { useState } from 'react';
import { Plus } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { useExecutionStore } from '@/features/workflows/stores/executionStore';
import { useEdgeContext } from './EdgeContext';

export function AnimatedSVGEdge({
  id,
  sourceX,
  sourceY,
  targetX,
  targetY,
  sourcePosition,
  targetPosition,
  source,
  target,
  sourceHandleId,
  selected,
}: EdgeProps) {
  const { onInsertClick } = useEdgeContext();
  const [isHovered, setIsHovered] = useState(false);

  // Check if workflow is executing (read-only mode)
  const isExecuting = useExecutionStore((state) => !!state.executingInstanceId);
  const [edgePath, labelX, labelY] = getBezierPath({
    sourceX,
    sourceY,
    sourcePosition,
    targetX,
    targetY,
    targetPosition,
  });

  // Determine label and color based on sourceHandle
  const label =
    sourceHandleId === 'true'
      ? 'True'
      : sourceHandleId === 'false'
        ? 'False'
        : '';

  const isDark = document.documentElement.classList.contains('dark');
  const baseStrokeColor =
    sourceHandleId === 'true'
      ? isDark
        ? '#10b98150'
        : '#10b981' // green-500 with 30% opacity in dark mode
      : sourceHandleId === 'false'
        ? isDark
          ? '#ef444450'
          : '#ef4444' // red-500 with 30% opacity in dark mode
        : sourceHandleId === 'onstart'
          ? isDark
            ? 'hsl(217, 91%, 60%, 0.3)'
            : 'hsl(217, 91%, 60%)' // primary color with opacity in dark mode
          : isDark
            ? 'hsl(214, 32%, 30%)'
            : 'hsl(214, 32%, 91%)'; // border color (darker in dark mode)

  // Adjust color based on state
  let strokeColor = baseStrokeColor;
  let strokeWidth = 1;

  if (selected) {
    // Make color brighter when selected
    strokeColor =
      sourceHandleId === 'true'
        ? isDark
          ? '#34d39970'
          : '#34d399' // green-400 with opacity in dark mode
        : sourceHandleId === 'false'
          ? isDark
            ? '#f8717170'
            : '#f87171' // red-400 with opacity in dark mode
          : sourceHandleId === 'onstart'
            ? isDark
              ? 'hsl(217, 91%, 70%, 0.4)'
              : 'hsl(217, 91%, 70%)' // brighter primary with opacity in dark mode
            : isDark
              ? 'hsl(214, 32%, 40%)'
              : 'hsl(214, 32%, 70%)'; // brighter border
    strokeWidth = 1.5;
  } else if (isHovered) {
    // Make color slightly brighter when hovered
    strokeColor =
      sourceHandleId === 'true'
        ? isDark
          ? '#22c55e60'
          : '#22c55e' // green-400 with opacity in dark mode
        : sourceHandleId === 'false'
          ? isDark
            ? '#f8717160'
            : '#f87171' // red-400 with opacity in dark mode
          : sourceHandleId === 'onstart'
            ? isDark
              ? 'hsl(217, 91%, 65%, 0.35)'
              : 'hsl(217, 91%, 65%)' // slightly brighter primary with opacity in dark mode
            : isDark
              ? 'hsl(214, 32%, 35%)'
              : 'hsl(214, 32%, 80%)'; // slightly brighter border
    strokeWidth = 1.25;
  }

  const handleInsertClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    if (onInsertClick) {
      onInsertClick({
        id,
        source,
        target,
        sourceHandle: sourceHandleId || 'source',
        position: { x: labelX, y: labelY },
      });
    }
  };

  return (
    <>
      <g
        onMouseEnter={() => setIsHovered(true)}
        onMouseLeave={() => setIsHovered(false)}
        style={{ cursor: 'pointer' }}
      >
        <BaseEdge
          id={id}
          path={edgePath}
          strokeWidth={strokeWidth}
          style={{
            stroke: strokeColor,
            transition: 'stroke 0.2s ease, stroke-width 0.2s ease',
          }}
        />
        {/* Invisible wider path for better hover detection */}
        <path
          d={edgePath}
          fill="none"
          stroke="transparent"
          strokeWidth={20}
          style={{ pointerEvents: 'stroke' }}
        />
      </g>
      <EdgeLabelRenderer>
        {label && (
          <div
            style={{
              position: 'absolute',
              transform: `translate(-50%, -50%) translate(${labelX}px,${labelY}px)`,
              pointerEvents: 'all',
              zIndex: 1002,
            }}
            className="nodrag nopan bg-background/80 backdrop-blur-sm px-1 py-px rounded text-[9px] font-normal text-muted-foreground/60"
          >
            {label}
          </div>
        )}
        {/* Show button when edge is selected and not executing */}
        {selected && onInsertClick && !isExecuting && (
          <div
            style={{
              position: 'absolute',
              transform: `translate(-50%, -50%) translate(${labelX}px,${labelY + (label ? 20 : 0)}px)`,
              pointerEvents: 'all',
              zIndex: 1000,
            }}
            className="nodrag nopan"
            onMouseEnter={() => setIsHovered(true)}
            onMouseLeave={() => setIsHovered(false)}
          >
            <Button
              className="w-4 h-4 rounded-full [&_svg]:size-2 shadow-sm hover:shadow-md"
              variant="outline"
              size="icon"
              onClick={handleInsertClick}
            >
              <Plus />
            </Button>
          </div>
        )}
      </EdgeLabelRenderer>
    </>
  );
}
