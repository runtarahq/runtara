/**
 * Read-only "Output" section for the step editor: shows what the step being
 * edited writes into `steps.<id>` so authors can wire downstream references
 * without running the workflow first.
 *
 * Sources, in order of specificity:
 * - Agent steps: the capability's declared output schema (meta.json).
 * - EmbedWorkflow steps: the child workflow's output schema.
 * - Control steps (Split, While, Filter, …): the canonical outputShape table
 *   (runtara-dsl step_output_shape via the validation WASM).
 * - Finish steps render their own output schema preview elsewhere.
 */
import { useContext, useEffect, useMemo, useState } from 'react';
import { useFormContext, useWatch } from 'react-hook-form';
import { Badge } from '@/shared/components/ui/badge';
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from '@/shared/components/ui/collapsible';
import { Icons } from '@/shared/components/icons';
import {
  getStepOutputShape,
  warmStepOutputShapes,
  OutputShapeJson,
} from '@/features/workflows/utils/step-output-shapes';
import { parseSchema } from '@/features/workflows/utils/schema';
import { NodeFormContext } from './NodeFormContext';

interface OutputFieldLike {
  name: string;
  type?: string;
  description?: string;
  nullable?: boolean;
  fields?: OutputFieldLike[];
  items?: { type?: string; fields?: OutputFieldLike[] };
}

function FieldRow({
  field,
  depth = 0,
}: {
  field: OutputFieldLike;
  depth?: number;
}) {
  const itemFields = field.items?.fields;
  return (
    <>
      <div
        className="flex items-center justify-between gap-2 rounded-md border border-border/50 bg-muted/30 px-3 py-1.5"
        style={depth > 0 ? { marginLeft: depth * 16 } : undefined}
        data-testid="output-field-row"
      >
        <div className="min-w-0 flex flex-col">
          <span className="font-mono text-xs truncate">{field.name}</span>
          {field.description && (
            <span className="text-[11px] text-muted-foreground truncate">
              {field.description}
            </span>
          )}
        </div>
        <div className="flex items-center gap-1 shrink-0">
          {field.nullable && (
            <Badge variant="secondary" className="h-5 rounded text-[10px]">
              nullable
            </Badge>
          )}
          <Badge variant="outline" className="h-5 rounded px-2 text-[10px]">
            {field.type && field.type !== 'dynamic' ? field.type : 'unknown'}
          </Badge>
        </div>
      </div>
      {(field.fields ?? []).map((child) => (
        <FieldRow key={`${field.name}.${child.name}`} field={child} depth={depth + 1} />
      ))}
      {itemFields &&
        itemFields.map((child) => (
          <FieldRow
            key={`${field.name}[].${child.name}`}
            field={{ ...child, name: `[item].${child.name}` }}
            depth={depth + 1}
          />
        ))}
    </>
  );
}

function referenceHint(stepId: string | undefined, tail: string) {
  return `steps.${stepId || '<id>'}${tail}`;
}

/** Body for Agent steps: capability output schema. */
function AgentOutputBody({ stepId }: { stepId?: string }) {
  const { agents } = useContext(NodeFormContext);
  const form = useFormContext();
  const agentId = useWatch({ name: 'agentId', control: form.control });
  const capabilityId = useWatch({
    name: 'capabilityId',
    control: form.control,
  });

  const output = useMemo(() => {
    const agent = agents.find((a) => a.id === agentId);
    const capability = agent?.supportedCapabilities?.[capabilityId ?? ''];
    return (capability?.output ?? null) as OutputFieldLike | null;
  }, [agents, agentId, capabilityId]);

  if (!output) {
    return (
      <p className="text-xs text-muted-foreground">
        No output schema declared for this capability.
      </p>
    );
  }

  const fields = output.fields ?? output.items?.fields ?? [];
  const containerType = output.type ?? 'unknown';

  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2 text-xs text-muted-foreground">
        <span className="font-mono">{referenceHint(stepId, '.outputs')}</span>
        <Badge variant="outline" className="h-5 rounded px-2 text-[10px]">
          {containerType}
        </Badge>
      </div>
      {fields.length > 0 ? (
        <div className="space-y-1">
          {fields.map((field) => (
            <FieldRow key={field.name} field={field} />
          ))}
        </div>
      ) : (
        <p className="text-xs text-muted-foreground">
          The exact shape depends on the response payload.
        </p>
      )}
    </div>
  );
}

/** Body for EmbedWorkflow steps: the child workflow's output schema. */
function EmbedOutputBody({ stepId }: { stepId?: string }) {
  const { workflows } = useContext(NodeFormContext);
  const form = useFormContext();
  const childWorkflowId = useWatch({
    name: 'childWorkflowId',
    control: form.control,
  });

  const fields = useMemo(() => {
    const child = workflows.find((w) => w.id === childWorkflowId);
    if (!child?.outputSchema) {
      return [];
    }
    try {
      return parseSchema(child.outputSchema);
    } catch {
      return [];
    }
  }, [workflows, childWorkflowId]);

  return (
    <div className="space-y-2">
      <div className="text-xs text-muted-foreground font-mono">
        {referenceHint(stepId, '.outputs')}
      </div>
      {fields.length > 0 ? (
        <div className="space-y-1">
          {fields.map((field) => (
            <FieldRow
              key={field.name}
              field={{
                name: field.name,
                type: field.type,
                description: field.description,
              }}
            />
          ))}
        </div>
      ) : (
        <p className="text-xs text-muted-foreground">
          The child workflow declares no output schema — outputs are the
          child&apos;s Finish step payload.
        </p>
      )}
    </div>
  );
}

/** Body for control steps: the canonical outputShape table. */
function ShapeOutputBody({
  stepType,
  stepId,
}: {
  stepType: string;
  stepId?: string;
}) {
  const [shape, setShape] = useState<OutputShapeJson | null>(() =>
    getStepOutputShape(stepType)
  );
  useEffect(() => {
    let cancelled = false;
    void warmStepOutputShapes().then(() => {
      if (!cancelled) {
        setShape(getStepOutputShape(stepType));
      }
    });
    return () => {
      cancelled = true;
    };
  }, [stepType]);

  if (!shape) {
    return (
      <p className="text-xs text-muted-foreground">
        No output information available for this step type.
      </p>
    );
  }

  const kind = shape.outputs?.kind;

  return (
    <div className="space-y-2">
      {shape.summary && (
        <p className="text-xs text-muted-foreground">
          {shape.summary.replace(/`/g, '')}
        </p>
      )}
      <div className="flex items-center gap-2 text-xs text-muted-foreground">
        <span className="font-mono">{referenceHint(stepId, '.outputs')}</span>
        {kind && (
          <Badge variant="outline" className="h-5 rounded px-2 text-[10px]">
            {kind === 'dynamic' ? 'runtime-dependent' : kind}
          </Badge>
        )}
      </div>
      {kind === 'object' && (shape.outputs?.fields?.length ?? 0) > 0 && (
        <div className="space-y-1">
          {shape.outputs!.fields!.map((field) => (
            <FieldRow
              key={field.name}
              field={{
                name: field.name,
                type: field.type,
                description: field.description,
              }}
            />
          ))}
        </div>
      )}
      {(shape.siblingFields?.length ?? 0) > 0 && (
        <div className="space-y-1">
          <p className="text-[11px] uppercase tracking-wide text-muted-foreground pt-1">
            Also written under steps.{stepId || '<id>'}
          </p>
          {shape.siblingFields!.map((field) => (
            <FieldRow
              key={field.name}
              field={{
                name: field.name,
                type: field.type,
                description: field.description,
              }}
            />
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * The collapsible Output section. Hidden for step types that either have
 * their own output editor (Finish) or aren't rendered as form steps (Start).
 */
export function StepOutputPanel() {
  const { nodeId } = useContext(NodeFormContext);
  const form = useFormContext();
  const stepType = useWatch({ name: 'stepType', control: form.control });
  const capabilityId = useWatch({
    name: 'capabilityId',
    control: form.control,
  });
  const [open, setOpen] = useState(false);

  if (!stepType || stepType === 'Start' || stepType === 'Finish') {
    return null;
  }
  // Agent steps without a chosen capability have nothing to describe yet.
  if (stepType === 'Agent' && !capabilityId) {
    return null;
  }

  return (
    <Collapsible open={open} onOpenChange={setOpen}>
      <CollapsibleTrigger
        type="button"
        className="flex w-full items-center gap-1.5 text-sm font-medium py-1 text-left"
        data-testid="step-output-panel-trigger"
      >
        <Icons.chevronRight
          className={`h-3.5 w-3.5 transition-transform ${open ? 'rotate-90' : ''}`}
        />
        Output
        <span className="text-xs font-normal text-muted-foreground">
          — what this step produces
        </span>
      </CollapsibleTrigger>
      <CollapsibleContent className="pt-1 pb-2">
        {stepType === 'Agent' ? (
          <AgentOutputBody stepId={nodeId} />
        ) : stepType === 'EmbedWorkflow' ? (
          <EmbedOutputBody stepId={nodeId} />
        ) : (
          <ShapeOutputBody stepType={stepType} stepId={nodeId} />
        )}
      </CollapsibleContent>
    </Collapsible>
  );
}
