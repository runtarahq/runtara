import { useNavigate, useParams } from 'react-router';
import { toast } from 'sonner';
import { useCustomMutation, useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { ScenarioDto } from '@/generated/RuntaraRuntimeApi';
import { Loader } from '@/shared/components/loader.tsx';
import { TriggerForm } from '@/features/triggers/components/TriggerForm';
import { queryClient } from '@/main';
import { scheduleToCron, cronToSchedule } from '@/features/triggers/utils/cron';
import {
  getInvocationTriggerById,
  updateInvocationTrigger,
} from '@/features/triggers/queries';
import { getScenarios } from '@/features/scenarios/queries';
import { getConnections } from '@/features/connections/queries';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { defaultScheduleConfig } from '@/features/triggers/components/TriggerForm/TriggerItem';

export function EditTrigger() {
  const { triggerId } = useParams();

  const navigate = useNavigate();

  const trigger = useCustomQuery({
    queryKey: queryKeys.triggers.byId(triggerId ?? ''),
    queryFn: (token: string) => getInvocationTriggerById(token, triggerId!),
    enabled: !!triggerId,
  });

  // Set page title with trigger name
  const triggerData = trigger.data as any;
  usePageTitle(
    triggerData?.name
      ? `Edit Trigger - ${triggerData.name}`
      : 'Edit Invocation Trigger'
  );

  const scenarios = useCustomQuery({
    queryKey: queryKeys.scenarios.all,
    queryFn: getScenarios,
    select: (response: any) => {
      const scenariosData = response?.data?.content || [];
      return scenariosData.map((scenario: ScenarioDto) => ({
        id: scenario.id,
        name: scenario.name,
      }));
    },
  });

  const connectionsQuery = useCustomQuery({
    queryKey: queryKeys.connections.all,
    queryFn: getConnections,
  });

  const updateMutation = useCustomMutation({
    mutationFn: updateInvocationTrigger,
    onSuccess: () => {
      navigate('/invocation-triggers');
      toast.info('Invocation Trigger has been updated.');
      queryClient.invalidateQueries({
        queryKey: queryKeys.triggers.all,
      });
    },
    onError: (error: any) => {
      console.error('updateMutation onError callback:', error);
      toast.error('Failed to update Invocation Trigger. Please try again.');
    },
  });

  const handleSubmit = (data: any) => {
    const {
      scheduleConfig,
      triggerType,
      applicationName,
      eventType,
      connectionId,
      sessionMode,
      configuration,
      ...restTrigger
    } = data;

    let finalConfiguration = null;

    switch (triggerType) {
      case 'CRON':
        if (scheduleConfig) {
          finalConfiguration = { expression: scheduleToCron(scheduleConfig) };
        }
        break;
      case 'APPLICATION':
        finalConfiguration = {
          ...(configuration || {}),
          applicationName,
          eventType,
        };
        break;
      case 'CHANNEL':
        finalConfiguration = {
          ...(configuration || {}),
          connection_id: connectionId || (configuration as any)?.connection_id,
          ...(sessionMode && sessionMode !== 'per_sender'
            ? { session_mode: sessionMode }
            : {}),
        };
        break;
      default:
        finalConfiguration = null;
        break;
    }

    try {
      updateMutation.mutate({
        ...restTrigger,
        id: triggerId,
        triggerType,
        configuration: finalConfiguration,
      });
    } catch (error) {
      console.error('Error in updateMutation.mutate:', error);
    }
  };

  if (
    trigger.isFetching ||
    scenarios.isFetching ||
    connectionsQuery.isFetching
  ) {
    return <Loader />;
  }

  // Prepare initial values with schedule config conversion
  const initValues = { ...trigger.data } as any;

  // Convert cron expression to ScheduleConfig for CRON triggers
  if (
    initValues.triggerType === 'CRON' &&
    initValues.configuration?.expression
  ) {
    initValues.scheduleConfig = cronToSchedule(
      initValues.configuration.expression
    );
  } else {
    initValues.scheduleConfig = defaultScheduleConfig;
  }

  // Extract connectionId and sessionMode for CHANNEL triggers
  if (initValues.triggerType === 'CHANNEL') {
    if (initValues.configuration?.connection_id) {
      initValues.connectionId = initValues.configuration.connection_id;
    }
    initValues.sessionMode =
      initValues.configuration?.session_mode || 'per_sender';
  }

  // Ensure triggerType is set to APPLICATION if configuration exists and has applicationName
  if (initValues.configuration && initValues.configuration.applicationName) {
    initValues.triggerType = 'APPLICATION';

    // Also ensure applicationName and eventType are set at the top level
    if (initValues.configuration.applicationName) {
      initValues.applicationName = initValues.configuration.applicationName;
    }

    if (initValues.configuration.eventType) {
      initValues.eventType = initValues.configuration.eventType;
    }
  }

  const metadata = [
    trigger.data?.id ? `ID: ${trigger.data.id}` : null,
    trigger.data?.updatedAt
      ? `Updated ${new Date(trigger.data.updatedAt).toLocaleString()}`
      : null,
  ].filter(Boolean) as string[];

  return (
    <div className="w-full px-4 py-6 sm:px-6 lg:px-10">
      <div className="mx-auto flex w-full max-w-5xl flex-col gap-6">
        <section className="space-y-3 px-4 sm:px-5">
          <p className="text-xs font-semibold uppercase tracking-[0.08em] text-muted-foreground">
            Invocation triggers
          </p>
          <div className="space-y-2">
            <h1 className="text-3xl font-semibold leading-tight text-slate-900/90">
              Edit trigger
            </h1>
            <p className="text-sm text-muted-foreground">
              Update when this trigger runs and keep its configuration in sync.
            </p>
          </div>
          {metadata.length > 0 && (
            <div className="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
              {metadata.map((item) => (
                <span key={item}>{item}</span>
              ))}
            </div>
          )}
        </section>

        <section className="space-y-4 px-4 sm:px-5">
          <TriggerForm
            title="Trigger details"
            description="Adjust the scenario mapping, schedule, and application payloads."
            fieldProps={{
              scenarios: scenarios.data,
              connections: connectionsQuery.data,
            }}
            initValues={initValues}
            isLoading={updateMutation.isPending}
            submitLabel="Save changes"
            loadingLabel="Saving..."
            onSubmit={handleSubmit}
          />
        </section>
      </div>
    </div>
  );
}
