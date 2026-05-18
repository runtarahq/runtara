import { useMemo } from 'react';
import { useFormContext } from 'react-hook-form';
import { CheckCircle2 } from 'lucide-react';
import { AgentSummary, ConnectionTypeDto } from '@/generated/RuntaraRuntimeApi';
import { RuntimeREST } from '@/shared/queries';
import { createAuthHeaders } from '@/shared/queries/utils';
import { useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { Checkbox } from '@/shared/components/ui/checkbox';
import { Label } from '@/shared/components/ui/label';
import { FormSection } from './FormSection';

const OBJECT_STORAGE_DEFAULT_FOR = 'object_storage';
const OBJECT_STORAGE_INTEGRATION_IDS = new Set([
  's3_compatible',
  'azure_blob_storage',
]);
const FILE_STORAGE_CATEGORIES = new Set(['file_storage', 'storage']);

interface DefaultTarget {
  id: string;
  label: string;
}

async function listAgentSummaries(token: string) {
  const result = await RuntimeREST.api.listAgentsHandler(
    createAuthHeaders(token)
  );
  return result.data.agents ?? [];
}

export function DefaultForSection({
  connectionType,
}: {
  connectionType: ConnectionTypeDto;
}) {
  const form = useFormContext();
  const defaultFor = (form.watch('defaultFor') ?? []) as string[];
  const integrationId = connectionType.integrationId;
  const supportsObjectStorageDefault =
    OBJECT_STORAGE_INTEGRATION_IDS.has(integrationId) ||
    FILE_STORAGE_CATEGORIES.has(connectionType.category ?? '');

  const agentsQuery = useCustomQuery<AgentSummary[]>({
    queryKey: queryKeys.agents.lists(),
    queryFn: listAgentSummaries,
    placeholderData: [],
  });

  const targets = useMemo<DefaultTarget[]>(() => {
    const agentTargets =
      agentsQuery.data
        ?.filter(
          (agent) =>
            agent.supportsConnections &&
            agent.integrationIds.includes(integrationId)
        )
        .map((agent) => ({
          id: agent.id,
          label: agent.name || agent.id,
        })) ?? [];

    if (!supportsObjectStorageDefault) return agentTargets;
    return [
      ...agentTargets,
      { id: OBJECT_STORAGE_DEFAULT_FOR, label: 'Object storage' },
    ];
  }, [agentsQuery.data, integrationId, supportsObjectStorageDefault]);

  if (targets.length === 0) return null;

  const toggleDefault = (targetId: string, checked: boolean) => {
    const next = new Set(defaultFor);
    if (checked) next.add(targetId);
    else next.delete(targetId);
    form.setValue('defaultFor', Array.from(next).sort(), {
      shouldDirty: true,
      shouldTouch: true,
    });
  };

  return (
    <FormSection title="Defaults" icon={CheckCircle2} optional>
      <div className="grid gap-3">
        {targets.map((target) => {
          const checked = defaultFor.includes(target.id);
          return (
            <div
              key={target.id}
              className="flex items-center gap-3 rounded-lg border bg-background px-3 py-3"
            >
              <Checkbox
                id={`default-for-${target.id}`}
                checked={checked}
                onCheckedChange={(value) =>
                  toggleDefault(target.id, value === true)
                }
              />
              <Label
                htmlFor={`default-for-${target.id}`}
                className="text-sm font-medium"
              >
                {target.label}
              </Label>
            </div>
          );
        })}
      </div>
    </FormSection>
  );
}
