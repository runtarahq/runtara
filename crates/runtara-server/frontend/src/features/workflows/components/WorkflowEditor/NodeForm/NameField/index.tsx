import { useContext, useMemo, useState } from 'react';
import { useFormContext, useWatch, FieldValues } from 'react-hook-form';
import { Check, Pencil, X } from 'lucide-react';
import {
  FormControl,
  FormField as FormFieldUI,
  FormItem,
  FormMessage,
} from '@/shared/components/ui/form';
import { Input } from '@/shared/components/ui/input';
import { useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { getConnectionsByOperator } from '@/features/connections/queries';
import { ConnectionDto } from '@/generated/RuntaraRuntimeApi';
import { ExtendedAgent } from '@/features/workflows/queries';
import {
  getStepIdValidationError,
  useWorkflowStore,
} from '@/features/workflows/stores/workflowStore';
import { NodeFormContext } from '../NodeFormContext';
import { CapabilityPickerModal } from '../CapabilityPickerModal';
import { ConnectionPickerModal } from '../ConnectionPickerModal';

/**
 * Monospace step-id row with an inline rename editor. Renames apply to the
 * workflow store immediately (steps ids are not part of the form schema):
 * the store re-points edges/children and rewrites all step references.
 */
function StepIdRow({ stepId }: { stepId: string }) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(stepId);
  const [error, setError] = useState<string | null>(null);

  // Only render for steps that already exist on the canvas (the create
  // dialog passes a provisional id that is not in the store yet).
  const nodeExists = useWorkflowStore((state) =>
    state.nodes.some((node) => node.id === stepId)
  );
  if (!nodeExists) {
    return null;
  }

  const validateDraft = (value: string): string | null => {
    if (value === stepId) return null; // unchanged: confirming is a no-op
    const ids = useWorkflowStore
      .getState()
      .nodes.map((node) => node.id as string);
    return getStepIdValidationError(value, stepId, ids);
  };

  const startEditing = () => {
    setDraft(stepId);
    setError(null);
    setEditing(true);
  };

  const cancelEditing = () => {
    setDraft(stepId);
    setError(null);
    setEditing(false);
  };

  const confirmEditing = () => {
    const next = draft.trim();
    if (next === stepId) {
      cancelEditing();
      return;
    }
    const validationError = validateDraft(next);
    if (validationError) {
      setError(validationError);
      return;
    }
    const renameError = useWorkflowStore
      .getState()
      .renameStep(stepId, next);
    if (renameError) {
      setError(renameError);
      return;
    }
    setEditing(false);
    setError(null);
  };

  if (!editing) {
    return (
      <div className="space-y-0.5">
        <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <span className="font-mono truncate" title={stepId}>
            {stepId}
          </span>
          <button
            type="button"
            aria-label="Edit step id"
            onClick={startEditing}
            className="shrink-0 text-muted-foreground hover:text-primary"
          >
            <Pencil className="h-3 w-3" />
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-1">
      <div className="flex items-center gap-1.5">
        <Input
          value={draft}
          autoFocus
          spellCheck={false}
          aria-label="Step id"
          onChange={(event) => {
            const value = event.target.value;
            setDraft(value);
            setError(validateDraft(value.trim()));
          }}
          onKeyDown={(event) => {
            if (event.key === 'Enter') {
              event.preventDefault();
              confirmEditing();
            } else if (event.key === 'Escape') {
              event.preventDefault();
              cancelEditing();
            }
          }}
          className="h-7 px-2 font-mono text-xs"
        />
        <button
          type="button"
          aria-label="Confirm step id"
          onClick={confirmEditing}
          disabled={Boolean(error)}
          className="shrink-0 text-muted-foreground hover:text-primary disabled:opacity-40"
        >
          <Check className="h-3.5 w-3.5" />
        </button>
        <button
          type="button"
          aria-label="Cancel step id edit"
          onClick={cancelEditing}
          className="shrink-0 text-muted-foreground hover:text-destructive"
        >
          <X className="h-3.5 w-3.5" />
        </button>
      </div>
      {error ? (
        <p className="text-xs text-destructive">{error}</p>
      ) : (
        <p className="text-xs text-muted-foreground">
          Used in reference paths (steps.{draft.trim() || '<id>'}
          .outputs...). Renaming rewrites all references.
        </p>
      )}
    </div>
  );
}

export function NameField({ name }: { name: string }) {
  const form = useFormContext();
  const { agents, nodeId }: FieldValues = useContext(NodeFormContext);
  const [connectionPickerOpen, setConnectionPickerOpen] = useState(false);
  const [capabilityPickerOpen, setCapabilityPickerOpen] = useState(false);

  const stepType = useWatch({ name: 'stepType', control: form.control });
  const capabilityId = useWatch({
    name: 'capabilityId',
    control: form.control,
  });
  const agentId = useWatch({ name: 'agentId', control: form.control });
  const connectionId = useWatch({
    name: 'connectionId',
    control: form.control,
  });

  // Get agent info (case-insensitive lookup to handle legacy data)
  const { id: agentModuleId, supportsConnections } = useMemo(() => {
    const agentIdLower = agentId?.toLowerCase();
    const agent =
      agents?.find(
        (ag: ExtendedAgent) => ag.id.toLowerCase() === agentIdLower
      ) || {};
    return agent;
  }, [agents, agentId]);

  // Fetch connections for agent — use agent.id (module id), not display name
  const connectionsByAgent = useCustomQuery({
    queryKey: queryKeys.agents.connectionsByAgent(agentModuleId ?? ''),
    queryFn: (token: string) => getConnectionsByOperator(token, agentModuleId!),
    placeholderData: [],
    enabled: Boolean(agentModuleId && supportsConnections),
  });

  // Get selected connection name
  const selectedConnection = useMemo(() => {
    if (!connectionId) return null;
    const connections = connectionsByAgent.data || [];
    return connections.find((c: ConnectionDto) => c.id === connectionId);
  }, [connectionId, connectionsByAgent.data]);

  // Hide until capability is selected for Agent steps
  if (stepType === 'Agent' && !capabilityId) {
    return null;
  }

  const handleConnectionChange = (value: string) => {
    form.setValue('connectionId', value);
  };

  const handleCapabilitySelect = (
    newAgentId: string,
    newCapabilityId: string
  ) => {
    // Update agent if changed
    if (newAgentId !== agentId) {
      form.setValue('agentId', newAgentId);
      form.setValue('connectionId', ''); // Reset connection when agent changes
    }
    // Update capability and clear input mapping
    form.setValue('capabilityId', newCapabilityId, { shouldValidate: true });
    form.setValue('inputMapping', []);
  };

  // Connection selector link
  const connectionSelector = supportsConnections && (
    <button
      type="button"
      onClick={() => setConnectionPickerOpen(true)}
      className="text-primary hover:text-primary/80 hover:underline"
    >
      {selectedConnection?.title || 'Select connection'}
    </button>
  );

  // Agent steps: editable name as title + agent/capability/connection subtitle
  if (stepType === 'Agent' && capabilityId) {
    const agent = agents?.find((ag: any) => ag.id === agentId);

    return (
      <div className="space-y-1">
        {/* Title row: name */}
        <FormFieldUI
          control={form.control}
          name={name}
          render={({ field }) => (
            <FormItem className="space-y-0">
              <FormControl>
                <Input
                  {...field}
                  placeholder="Step name"
                  className="h-auto text-lg font-semibold text-slate-900/90 dark:text-slate-100 border-0 bg-transparent p-0 focus-visible:ring-0 focus-visible:ring-offset-0"
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        {/* Agent → capability • connection subtitle - all clickable */}
        <div className="flex items-center gap-2 text-sm text-muted-foreground">
          <button
            type="button"
            onClick={() => setCapabilityPickerOpen(true)}
            className="text-primary hover:text-primary/80 hover:underline"
          >
            {agent?.name || agentId} → {capabilityId}
          </button>
          {supportsConnections && (
            <>
              <span>•</span>
              {connectionSelector}
            </>
          )}
        </div>

        {/* Step id row with inline rename */}
        {nodeId && <StepIdRow stepId={nodeId} />}

        {/* Capability Picker Modal */}
        <CapabilityPickerModal
          open={capabilityPickerOpen}
          onOpenChange={setCapabilityPickerOpen}
          onSelect={handleCapabilitySelect}
          currentAgentId={agentId}
          currentCapabilityId={capabilityId}
        />

        {/* Connection Picker Modal */}
        <ConnectionPickerModal
          open={connectionPickerOpen}
          onOpenChange={setConnectionPickerOpen}
          onSelect={handleConnectionChange}
          connections={connectionsByAgent.data || []}
          currentConnectionId={connectionId}
        />
      </div>
    );
  }

  // Non-Agent steps: name
  return (
    <div className="space-y-1">
      <FormFieldUI
        control={form.control}
        name={name}
        render={({ field }) => (
          <FormItem className="space-y-0">
            <FormControl>
              <Input
                {...field}
                placeholder="Step name"
                className="h-auto text-lg font-semibold text-slate-900/90 dark:text-slate-100 border-0 bg-transparent p-0 focus-visible:ring-0 focus-visible:ring-offset-0"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />

      {/* Step id row with inline rename */}
      {nodeId && <StepIdRow stepId={nodeId} />}
    </div>
  );
}
