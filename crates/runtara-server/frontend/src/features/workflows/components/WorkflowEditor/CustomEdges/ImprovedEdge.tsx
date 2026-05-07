import {
  BaseEdge,
  EdgeLabelRenderer,
  type EdgeProps,
  getBezierPath,
  getSmoothStepPath,
  Position,
} from '@xyflow/react';
import { useMemo, useState } from 'react';
import { Plus, Wrench, Brain } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { useExecutionStore } from '@/features/workflows/stores/executionStore';
import { useEdgeContext } from './EdgeContext';
import { shouldHideDuplicateEdgeLabel } from '../CustomNodes/layout';

interface ImprovedEdgeProps extends EdgeProps {
  pathType?: 'bezier' | 'smoothstep' | 'straight';
}

function getOrthogonalPath(points: Array<{ x: number; y: number }>): string {
  if (points.length === 0) return '';
  if (points.length === 1) return `M ${points[0].x},${points[0].y}`;

  const cornerRadius = 8;
  let path = `M ${points[0].x},${points[0].y}`;

  for (let index = 1; index < points.length - 1; index++) {
    const previous = points[index - 1];
    const current = points[index];
    const next = points[index + 1];
    const incoming = {
      x: Math.sign(current.x - previous.x),
      y: Math.sign(current.y - previous.y),
    };
    const outgoing = {
      x: Math.sign(next.x - current.x),
      y: Math.sign(next.y - current.y),
    };
    const previousDistance =
      Math.abs(current.x - previous.x) + Math.abs(current.y - previous.y);
    const nextDistance =
      Math.abs(next.x - current.x) + Math.abs(next.y - current.y);
    const isCorner =
      incoming.x !== outgoing.x || incoming.y !== outgoing.y;
    const radius = Math.min(
      cornerRadius,
      previousDistance / 2,
      nextDistance / 2
    );

    if (!isCorner || radius <= 0) {
      path += ` L ${current.x},${current.y}`;
      continue;
    }

    const cornerStart = {
      x: current.x - incoming.x * radius,
      y: current.y - incoming.y * radius,
    };
    const cornerEnd = {
      x: current.x + outgoing.x * radius,
      y: current.y + outgoing.y * radius,
    };

    path += ` L ${cornerStart.x},${cornerStart.y}`;
    path += ` Q ${current.x},${current.y} ${cornerEnd.x},${cornerEnd.y}`;
  }

  const last = points[points.length - 1];
  return `${path} L ${last.x},${last.y}`;
}

export function ImprovedEdge({
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
  markerEnd,
  pathType = 'smoothstep',
  selected,
}: ImprovedEdgeProps) {
  const { onInsertClick, allEdges, edgeRoutes } = useEdgeContext();
  const [isHovered, setIsHovered] = useState(false);

  // Check if workflow is executing (read-only mode)
  const isExecuting = useExecutionStore((state) => !!state.executingInstanceId);

  // Calculate edge path based on type
  let edgePath: string;
  let labelX: number;
  let labelY: number;
  const routedEdge = edgeRoutes[id];

  if (routedEdge?.points && routedEdge.points.length >= 2) {
    edgePath = getOrthogonalPath(routedEdge.points);
    labelX =
      routedEdge.labelPoint?.x ??
      routedEdge.points[Math.floor(routedEdge.points.length / 2)].x;
    labelY =
      routedEdge.labelPoint?.y ??
      routedEdge.points[Math.floor(routedEdge.points.length / 2)].y;
  } else if (pathType === 'bezier') {
    [edgePath, labelX, labelY] = getBezierPath({
      sourceX,
      sourceY,
      sourcePosition,
      targetX,
      targetY,
      targetPosition,
    });
  } else if (pathType === 'smoothstep') {
    // Use smooth step for better readability with minimal crossings
    const borderRadius = 8;
    const offset = 0;

    [edgePath, labelX, labelY] = getSmoothStepPath({
      sourceX,
      sourceY,
      sourcePosition: sourcePosition || Position.Right,
      targetX,
      targetY,
      targetPosition: targetPosition || Position.Left,
      borderRadius,
      offset,
    });
  } else {
    // Straight line for simple connections
    edgePath = `M ${sourceX},${sourceY} L ${targetX},${targetY}`;
    labelX = (sourceX + targetX) / 2;
    labelY = (sourceY + targetY) / 2;
  }

  // Check if both true and false edges from the same source go to the same target
  const hasSiblingEdge =
    (sourceHandleId === 'true' || sourceHandleId === 'false') &&
    allEdges.some(
      (edge) =>
        edge.source === source &&
        edge.target === target &&
        edge.id !== id &&
        (edge.sourceHandle === 'true' || edge.sourceHandle === 'false')
    );

  // Calculate label Y offset when sibling edges exist to prevent overlap
  // True label moves up, False label moves down
  const labelYOffset = hasSiblingEdge
    ? sourceHandleId === 'true'
      ? -12
      : sourceHandleId === 'false'
        ? 12
        : 0
    : 0;

  // Determine label and color based on sourceHandle
  // Show labels even when both go to the same target (they will be offset)
  const knownHandleIds = new Set([
    'source',
    'target',
    'true',
    'false',
    'onError',
    'default',
    'onstart',
  ]);
  const isMemoryEdge = sourceHandleId === 'memory';
  const isToolEdge =
    sourceHandleId != null &&
    !knownHandleIds.has(sourceHandleId) &&
    !sourceHandleId.startsWith('case-') &&
    sourceHandleId !== 'memory';

  const label = shouldHideDuplicateEdgeLabel({
    sourceHandle: sourceHandleId,
  })
    ? ''
    : sourceHandleId === 'true'
      ? 'True'
      : sourceHandleId === 'false'
        ? 'False'
        : sourceHandleId === 'onError'
          ? 'Error'
          : isMemoryEdge
            ? 'memory'
            : isToolEdge
              ? sourceHandleId
              : '';

  const isDark = document.documentElement.classList.contains('dark');
  const isSwitchCase = sourceHandleId?.startsWith('case-');
  const isSwitchDefault = sourceHandleId === 'default';

  const baseStrokeColor =
    sourceHandleId === 'true'
      ? isDark
        ? '#10b98150'
        : '#10b981' // green-500 with 30% opacity in dark mode
      : sourceHandleId === 'false'
        ? isDark
          ? '#ef444450'
          : '#ef4444' // red-500 with 30% opacity in dark mode
        : sourceHandleId === 'onError'
          ? isDark
            ? '#ef444470'
            : '#ef4444' // red-500 for error edges (destructive)
          : sourceHandleId === 'onstart'
            ? isDark
              ? 'hsl(217, 91%, 60%, 0.3)'
              : 'hsl(217, 91%, 60%)' // primary color with opacity in dark mode
            : isSwitchCase
              ? isDark
                ? '#3b82f650'
                : '#3b82f6' // blue-500 for Switch case edges
              : isSwitchDefault
                ? isDark
                  ? '#6b728050'
                  : '#6b7280' // gray-500 for Switch default edge
                : isMemoryEdge
                  ? isDark
                    ? '#3b82f650'
                    : '#3b82f6' // blue-500 for memory edges
                  : isToolEdge
                    ? isDark
                      ? '#8b5cf650'
                      : '#8b5cf6' // violet-500 for AI Agent tool edges
                    : isDark
                      ? 'hsl(214, 32%, 50%)'
                      : 'hsl(214, 32%, 65%)'; // visible gray for edges (darker than border)

  // Adjust color based on state (more subtle for Notion/Linear style)
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
          : sourceHandleId === 'onError'
            ? isDark
              ? '#f8717180'
              : '#f87171' // red-400 for error edges (destructive)
            : sourceHandleId === 'onstart'
              ? isDark
                ? 'hsl(217, 91%, 70%, 0.4)'
                : 'hsl(217, 91%, 70%)' // brighter primary with opacity in dark mode
              : isSwitchCase
                ? isDark
                  ? '#60a5fa70'
                  : '#60a5fa' // blue-400 for selected Switch case edges
                : isSwitchDefault
                  ? isDark
                    ? '#9ca3af60'
                    : '#9ca3af' // gray-400 for selected Switch default edge
                  : isMemoryEdge
                    ? isDark
                      ? '#60a5fa70'
                      : '#60a5fa' // blue-400 for selected memory edges
                    : isToolEdge
                      ? isDark
                        ? '#a78bfa70'
                        : '#a78bfa' // violet-400 for selected AI Agent tool edges
                      : isDark
                        ? 'hsl(214, 32%, 60%)'
                        : 'hsl(214, 32%, 50%)'; // darker when selected
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
          : sourceHandleId === 'onError'
            ? isDark
              ? '#f8717175'
              : '#f87171' // red-400 for error edges (destructive)
            : sourceHandleId === 'onstart'
              ? 'hsl(217, 91%, 65%)' // slightly brighter primary
              : isSwitchCase
                ? isDark
                  ? '#60a5fa60'
                  : '#60a5fa' // blue-400 for hovered Switch case edges
                : isSwitchDefault
                  ? isDark
                    ? '#9ca3af50'
                    : '#9ca3af' // gray-400 for hovered Switch default edge
                  : isMemoryEdge
                    ? isDark
                      ? '#60a5fa60'
                      : '#60a5fa' // blue-400 for hovered memory edges
                    : isToolEdge
                      ? isDark
                        ? '#a78bfa60'
                        : '#a78bfa' // violet-400 for hovered AI Agent tool edges
                      : isDark
                        ? 'hsl(214, 32%, 55%)'
                        : 'hsl(214, 32%, 55%)'; // slightly darker on hover
    strokeWidth = 1.25;
  }

  // Use full opacity for better performance
  const opacity = 1;

  // Position route labels near the source handle so they identify the branch
  // without colliding with the target card or its left handle.
  const LABEL_GAP_FROM_SOURCE = 24;
  const sourceSegLabel = useMemo(() => {
    if (!label) return { x: labelX, y: labelY };

    try {
      const path = document.createElementNS(
        'http://www.w3.org/2000/svg',
        'path'
      );
      path.setAttribute('d', edgePath);
      const totalLen = path.getTotalLength();

      if (totalLen < 40) return { x: labelX, y: labelY };

      const at = Math.min(totalLen, LABEL_GAP_FROM_SOURCE);
      const pt = path.getPointAtLength(at);
      return { x: pt.x, y: pt.y };
    } catch {
      return { x: labelX, y: labelY };
    }
  }, [edgePath, label, labelX, labelY]);

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
          markerEnd={markerEnd}
          style={{
            stroke: strokeColor,
            strokeWidth: strokeWidth,
            opacity,
            strokeDasharray:
              sourceHandleId === 'onError'
                ? '5,5'
                : isMemoryEdge
                  ? '6,3'
                  : undefined,
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
              transform: `translate(0, -50%) translate(${sourceSegLabel.x}px,${sourceSegLabel.y + labelYOffset}px)`,
              pointerEvents: 'all',
              zIndex: 1002,
            }}
            className="nodrag nopan px-1 py-px rounded text-[9px] font-normal text-muted-foreground/60 flex items-center gap-0.5 bg-background/80 backdrop-blur-sm"
          >
            {isMemoryEdge && <Brain className="w-2.5 h-2.5" />}
            {isToolEdge && <Wrench className="w-2.5 h-2.5" />}
            {label}
          </div>
        )}
        {/* Show insert button when edge is selected (not for tool/memory edges — those are direct connections) */}
        {selected &&
          onInsertClick &&
          !isExecuting &&
          !isToolEdge &&
          !isMemoryEdge && (
            <div
              style={{
                position: 'absolute',
                transform: `translate(-50%, -50%) translate(${labelX}px,${labelY + (label ? 20 : 0)}px)`,
                pointerEvents: 'all',
                zIndex: 9999,
              }}
              className="nodrag nopan"
              onMouseEnter={() => setIsHovered(true)}
              onMouseLeave={() => setIsHovered(false)}
            >
              <Button
                className="w-4 h-4 rounded-full [&_svg]:size-2 shadow-sm hover:shadow-md transition-all"
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
