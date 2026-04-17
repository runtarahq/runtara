import * as RuntimeAPI from '@/generated/RuntaraRuntimeApi.ts';
import { EnrichedTrigger } from '@/features/triggers/types';
import { cronToHuman } from '@/features/triggers/utils/cron';
import { createScenarioNameMap } from '@/features/triggers/utils/scenario-enrichment';
import { getScenarios } from '@/features/scenarios/queries';
import { RuntimeREST } from '@/shared/queries';
import { createAuthHeaders } from '@/shared/queries/utils';

// Helper function to convert snake_case API response to camelCase EnrichedTrigger for UI
function transformTriggerFromAPI(
  trigger: RuntimeAPI.InvocationTrigger & { webhookUrl?: string | null },
  scenarioName?: string
): EnrichedTrigger {
  return {
    id: trigger.id,
    scenarioId: trigger.scenario_id,
    scenarioName: scenarioName || trigger.scenario_id,
    triggerType: trigger.trigger_type,
    configuration: trigger.configuration,
    active: trigger.active,
    singleInstance: trigger.single_instance,
    remoteTenantId: trigger.remote_tenant_id,
    tenantId: trigger.tenant_id,
    lastRun: trigger.last_run,
    createdAt: trigger.created_at,
    updatedAt: trigger.updated_at,
    webhookUrl: trigger.webhookUrl,
  };
}

// Helper function to convert camelCase UI data to snake_case for API
function transformTriggerToAPI(
  trigger: any
):
  | RuntimeAPI.CreateInvocationTriggerRequest
  | RuntimeAPI.UpdateInvocationTriggerRequest {
  return {
    scenario_id: trigger.scenarioId,
    trigger_type: trigger.triggerType,
    configuration: trigger.configuration || null,
    active: trigger.active ?? true,
    single_instance: trigger.singleInstance ?? false,
    remote_tenant_id: trigger.remoteTenantId || null,
  };
}

export async function getInvocationTriggers(token: string) {
  // Fetch triggers and scenarios in parallel
  const [triggersResult, scenariosResult] = await Promise.all([
    RuntimeREST.api.listInvocationTriggers(createAuthHeaders(token)),
    getScenarios(token),
  ]);

  const scenarioNameMap = createScenarioNameMap(scenariosResult);

  return triggersResult.data.data.map((trigger) =>
    transformTriggerFromAPI(trigger, scenarioNameMap.get(trigger.scenario_id))
  );
}

export async function getInvocationTriggerById(
  token: string,
  triggerId: string
) {
  // Fetch trigger and scenarios in parallel
  const [triggerResult, scenariosResult] = await Promise.all([
    RuntimeREST.api.getInvocationTrigger(triggerId, createAuthHeaders(token)),
    getScenarios(token),
  ]);

  const scenarioNameMap = createScenarioNameMap(scenariosResult);

  const triggerData = triggerResult.data.data;
  const trigger = transformTriggerFromAPI(
    triggerData,
    scenarioNameMap.get(triggerData.scenario_id)
  );
  const { configuration, ...restTrigger } = trigger;

  // Type cast configuration to access its properties
  const config = configuration as any;

  // Extract time and timeUnit for CRON triggers
  const humanCron = cronToHuman(config?.expression || '');

  // Extract applicationName and eventType for APPLICATION triggers
  const applicationData =
    restTrigger.triggerType === 'APPLICATION'
      ? {
          applicationName: config?.applicationName || '',
          eventType: config?.eventType || '',
        }
      : {};

  // Extract connectionId for CHANNEL triggers
  const channelData =
    restTrigger.triggerType === 'CHANNEL'
      ? { connectionId: config?.connection_id || '' }
      : {};

  return {
    ...restTrigger,
    configuration,
    ...humanCron,
    ...applicationData,
    ...channelData,
  };
}

export async function createInvocationTrigger(
  token: string,
  invocationTrigger: any
) {
  const requestData = transformTriggerToAPI(invocationTrigger);

  await RuntimeREST.api.createInvocationTrigger(
    requestData,
    createAuthHeaders(token)
  );
}

export async function removeInvocationTrigger(
  token: string,
  triggerId: string
) {
  await RuntimeREST.api.deleteInvocationTrigger(
    triggerId,
    createAuthHeaders(token)
  );
}

export async function updateInvocationTrigger(token: string, newTrigger: any) {
  const { id, ...restData } = newTrigger;
  const requestData = transformTriggerToAPI(
    restData
  ) as RuntimeAPI.UpdateInvocationTriggerRequest;

  await RuntimeREST.api.updateInvocationTrigger(
    id,
    requestData,
    createAuthHeaders(token)
  );
}
