/**
 * The rendered size of every fixed-size canvas node.
 *
 * The auto-layout places nodes and routes edges from these sizes, and the node
 * components render at exactly these sizes. Both sides must call this module:
 * when they disagree, cards overflow their layout box, sit on top of each
 * other and edges run underneath them.
 *
 * Only containers and notes are resizable; everything else is sized from its
 * type, its data and the canvas detail level. Sizes saved with a workflow are
 * never trusted for fixed-size nodes — they go stale whenever the card design
 * changes.
 */
import {
  NODE_TYPE_SIZES,
  NODE_TYPES,
} from '@/features/workflows/config/workflow.ts';
import { SNAP_GRID_SIZE } from '@/features/workflows/config/workflow-editor';
import { cardDetailLayout, stepDetailRows } from '../step-details';

export type CanvasDetailLevel = 'compact' | 'detailed';

type Size = { width: number; height: number };

export const COMPACT_CARD_HEIGHT = NODE_TYPE_SIZES[NODE_TYPES.BasicNode].height;
export const COMPACT_CARD_WIDTH = NODE_TYPE_SIZES[NODE_TYPES.BasicNode].width;
/** Detailed cards are wider so mapped values stay readable. */
export const DETAILED_CARD_WIDTH = 264;
export const DETAIL_LINE_HEIGHT = 16;
const DETAIL_BLOCK_PADDING = 8;

export const SWITCH_HANDLE_SPACING = 24;
export const SWITCH_FIRST_HANDLE_TOP = 28;

// AiAgent card geometry. AiAgentNode renders these exact heights.
export const AI_AGENT_HEADER_HEIGHT = 32;
export const AI_AGENT_SECTION_LABEL_HEIGHT = 12;
export const AI_AGENT_ROW_HEIGHT = 22;
const AI_AGENT_TOOLS_PADDING = 6 + 4; // pt-1.5 + pb-1
const AI_AGENT_MEMORY_SECTION =
  1 + 4 + AI_AGENT_SECTION_LABEL_HEIGHT + AI_AGENT_ROW_HEIGHT + 6; // border, pt-1, label, row, pb-1.5

/**
 * Smallest odd multiple of the grid (36, 60, 84, ...) that fits `height`.
 * Odd multiples keep every node's vertical center on the same grid phase, so
 * a chain of nodes with different heights still lines up handle to handle.
 */
export function oddGridCeil(height: number): number {
  const step = SNAP_GRID_SIZE * 2;
  const k = Math.max(0, Math.ceil((height - SNAP_GRID_SIZE) / step));
  return k * step + SNAP_GRID_SIZE;
}

/** Height of the detail list under a card header; whole grid steps. */
export function detailBlockHeight(lineCount: number): number {
  if (lineCount <= 0) return 0;
  const step = SNAP_GRID_SIZE * 2;
  return (
    Math.ceil((lineCount * DETAIL_LINE_HEIGHT + DETAIL_BLOCK_PADDING) / step) *
    step
  );
}

export function isResizableNodeType(nodeType?: string): boolean {
  return (
    nodeType === NODE_TYPES.ContainerNode || nodeType === NODE_TYPES.NoteNode
  );
}

type NodeData = {
  stepType?: string;
  inputMapping?: unknown;
  [key: string]: unknown;
};

function mappingValue(data: NodeData | undefined, key: string): unknown {
  const inputMapping = data?.inputMapping;
  if (!Array.isArray(inputMapping)) return undefined;
  return inputMapping.find(
    (item: { type?: string } | null) => item && item.type === key
  )?.value;
}

export function getSwitchRouting(data: NodeData | undefined): {
  routing: boolean;
  caseCount: number;
} {
  const casesValue = mappingValue(data, 'cases');
  const cases = Array.isArray(casesValue) ? casesValue : [];
  const routing =
    mappingValue(data, 'routingMode') === true ||
    cases.some(
      (caseItem: { route?: string } | null) =>
        caseItem?.route && caseItem.route !== ''
    );
  return { routing, caseCount: cases.length };
}

export function getAiAgentToolCount(data: NodeData | undefined): number {
  const tools = mappingValue(data, 'tools');
  return Array.isArray(tools) ? tools.length : 0;
}

/** AiAgent card height: header, one row per tool plus "Add tool", memory. */
export function aiAgentNodeHeight(toolCount: number): number {
  const toolsSection =
    AI_AGENT_TOOLS_PADDING +
    AI_AGENT_SECTION_LABEL_HEIGHT +
    (toolCount + 1) * AI_AGENT_ROW_HEIGHT;
  return oddGridCeil(
    AI_AGENT_HEADER_HEIGHT + toolsSection + AI_AGENT_MEMORY_SECTION
  );
}

/** Height of a Switch in routing mode: one handle per case plus default. */
export function switchRoutingHeight(caseCount: number): number {
  const lastHandleTop =
    SWITCH_FIRST_HANDLE_TOP + caseCount * SWITCH_HANDLE_SPACING;
  return Math.max(
    COMPACT_CARD_HEIGHT,
    oddGridCeil(lastHandleTop + SWITCH_HANDLE_SPACING)
  );
}

/** Number of detail lines a card shows at this detail level. */
export function cardDetailLineCount(
  data: NodeData | undefined,
  detailLevel: CanvasDetailLevel
): number {
  if (detailLevel !== 'detailed' || !data) return 0;
  return cardDetailLayout(stepDetailRows(data)).lineCount;
}

/**
 * The size a fixed-size node renders at. Returns null for resizable nodes,
 * whose size is their own.
 */
export function getStepNodeSize(
  nodeType: string | undefined,
  data: NodeData | undefined,
  detailLevel: CanvasDetailLevel = 'compact'
): Size | null {
  const type = nodeType || NODE_TYPES.BasicNode;
  const cardWidth =
    detailLevel === 'detailed' ? DETAILED_CARD_WIDTH : COMPACT_CARD_WIDTH;

  switch (type) {
    case NODE_TYPES.AiAgentNode:
      return {
        width: NODE_TYPE_SIZES[NODE_TYPES.AiAgentNode].width,
        height: aiAgentNodeHeight(getAiAgentToolCount(data)),
      };
    case NODE_TYPES.SwitchNode: {
      const { routing, caseCount } = getSwitchRouting(data);
      if (routing) {
        return { width: cardWidth, height: switchRoutingHeight(caseCount) };
      }
      return {
        width: cardWidth,
        height:
          COMPACT_CARD_HEIGHT +
          detailBlockHeight(cardDetailLineCount(data, detailLevel)),
      };
    }
    case NODE_TYPES.BasicNode:
    case NODE_TYPES.ConditionalNode:
    case NODE_TYPES.CreateNode:
      return {
        width: cardWidth,
        height:
          COMPACT_CARD_HEIGHT +
          detailBlockHeight(cardDetailLineCount(data, detailLevel)),
      };
    case NODE_TYPES.StartIndicatorNode:
      return { ...NODE_TYPE_SIZES[NODE_TYPES.StartIndicatorNode] };
    default:
      return null;
  }
}
