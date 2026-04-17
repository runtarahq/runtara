import { useContext, useMemo, useState } from 'react';
import { useFormContext, useWatch, FieldValues } from 'react-hook-form';
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
import { ExtendedAgent } from '@/features/scenarios/queries';
import { NodeFormContext } from '../NodeFormContext';
import { CapabilityPickerModal } from '../CapabilityPickerModal';
import { ConnectionPickerModal } from '../ConnectionPickerModal';

export function NameField({ name }: { name: string }) {
  const form = useFormContext();
  const { agents }: FieldValues = useContext(NodeFormContext);
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
  );
}
