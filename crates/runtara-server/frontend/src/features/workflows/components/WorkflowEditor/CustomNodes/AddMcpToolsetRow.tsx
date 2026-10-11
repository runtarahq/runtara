import { useState } from 'react';
import { Plug } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import { FieldError } from '@/shared/components/ui/form';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/shared/components/ui/popover';
import * as form from '@/features/workflows/components/WorkflowEditor/NodeForm/NodeFormItem.tsx';
import {
  generateStepId,
  useWorkflowStore,
} from '@/features/workflows/stores/workflowStore.ts';
import { cn } from '@/lib/utils.ts';
import { AI_AGENT_ROW_HEIGHT } from './node-size';

/**
 * "Add MCP toolset…" on an AI Agent card. Connects an `mcp.<toolset>` route
 * to a new MCP Agent step — the shape validation.rs expects (an Agent step
 * whose agentId is "mcp") — which exposes `<toolset>_search` and
 * `<toolset>_invoke` tools to the agent. The step is configured in the docked
 * panel like any other new step.
 */
export function AddMcpToolsetRow({
  agentNodeId,
  agentName,
  disabled,
}: {
  agentNodeId: string;
  agentName: string;
  disabled: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [toolsetName, setToolsetName] = useState('');
  const [error, setError] = useState<string | null>(null);

  const handleAdd = () => {
    // Server rules (validation.rs AiAgentMcpEdge*): the suffix after "mcp."
    // must be non-empty and unique per AiAgent step — no charset restriction.
    const toolset = toolsetName.trim();
    if (!toolset) {
      setError('Toolset name is required');
      return;
    }
    const { edges, nodes, setPendingNewNode } = useWorkflowStore.getState();
    const duplicate = edges.some(
      (edge) =>
        edge.source === agentNodeId && edge.sourceHandle === `mcp.${toolset}`
    );
    if (duplicate) {
      setError(`Toolset "${toolset}" is already connected`);
      return;
    }

    const agentNode = nodes.find((node) => node.id === agentNodeId);
    const name = `${toolset} MCP toolset`;
    setPendingNewNode({
      id: generateStepId(
        name,
        nodes.map((node) => node.id)
      ),
      data: {
        ...form.initialValues,
        stepType: 'Agent',
        name,
        agentId: 'mcp',
        capabilityId: 'mcp-tool-invoke',
      } as any,
      // Tool, memory and MCP targets are hidden on the auto-laid-out canvas,
      // so the position only matters for a later manual layout.
      position: agentNode?.position ?? { x: 0, y: 0 },
      parentId: agentNode?.parentId,
      sourceNodeId: agentNodeId,
      sourceHandle: `mcp.${toolset}`,
    });
    setOpen(false);
    setToolsetName('');
    setError(null);
  };

  return (
    <Popover
      open={open}
      onOpenChange={(nextOpen) => {
        setOpen(nextOpen);
        if (!nextOpen) {
          setToolsetName('');
          setError(null);
        }
      }}
    >
      <PopoverTrigger asChild>
        <button
          type="button"
          aria-hidden={disabled || undefined}
          tabIndex={disabled ? -1 : undefined}
          aria-label={`Add MCP toolset to ${agentName}`}
          data-testid="ai-agent-add-mcp-toolset"
          className={cn(
            'nodrag nopan -mx-0.5 flex w-[calc(100%+4px)] cursor-pointer items-center gap-1.5 rounded-sm px-0.5 text-left hover:bg-muted/30',
            disabled && 'pointer-events-none invisible'
          )}
          style={{ height: AI_AGENT_ROW_HEIGHT }}
          onClick={(event) => event.stopPropagation()}
        >
          <Plug
            className="size-2.5 flex-shrink-0 text-muted-foreground/60"
            aria-hidden="true"
          />
          <span className="flex-1 text-3xs italic text-muted-foreground/60">
            Add MCP toolset…
          </span>
        </button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        side="bottom"
        className="nodrag nopan w-72 space-y-3"
      >
        <div>
          <p className="text-sm font-semibold text-foreground">MCP toolset</p>
          <p className="text-xs text-muted-foreground">
            Exposes {'<toolset>'}_search and {'<toolset>'}_invoke tools to the
            agent via an mcp.{'<toolset>'} route.
          </p>
        </div>
        <div className="space-y-2">
          <Label htmlFor={`mcp-toolset-name-${agentNodeId}`}>
            Toolset name
          </Label>
          <Input
            id={`mcp-toolset-name-${agentNodeId}`}
            value={toolsetName}
            onChange={(event) => {
              setToolsetName(event.target.value);
              setError(null);
            }}
            onKeyDown={(event) => {
              if (event.key === 'Enter') {
                event.preventDefault();
                handleAdd();
              }
            }}
            placeholder="e.g. linear"
            data-testid="ai-agent-mcp-toolset-name"
          />
          {error && <FieldError>{error}</FieldError>}
        </div>
        <div className="flex justify-end gap-2">
          <Button
            type="button"
            variant="secondary"
            size="sm"
            onClick={() => setOpen(false)}
          >
            Cancel
          </Button>
          <Button
            type="button"
            size="sm"
            onClick={handleAdd}
            data-testid="ai-agent-mcp-toolset-confirm"
          >
            Add
          </Button>
        </div>
      </PopoverContent>
    </Popover>
  );
}
