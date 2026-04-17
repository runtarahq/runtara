import { useNavigate } from 'react-router';
import { toast } from 'sonner';
import { useCustomMutation, useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { ScenarioDto } from '@/generated/RuntaraRuntimeApi';
import { Loader } from '@/shared/components/loader.tsx';
import { TriggerForm } from '@/features/triggers/components/TriggerForm';
import { scheduleToCron } from '@/features/triggers/utils/cron';
import { createInvocationTrigger } from '@/features/triggers/queries';
import { getScenarios } from '@/features/scenarios/queries';
import { getConnections } from '@/features/connections/queries';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { queryClient } from '@/main.tsx';

export function CreateTrigger() {
  const navigate = useNavigate();
  usePageTitle('Create Invocation Trigger');

  const { data: scenarios, isFetching: fetchingScenarios } = useCustomQuery({
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

  const { data: connections, isFetching: fetchingConnections } = useCustomQuery(
    {
      queryKey: queryKeys.connections.all,
      queryFn: getConnections,
    }
  );

  const isFetching = fetchingScenarios || fetchingConnections;

  const { mutate, isPending } = useCustomMutation({
    mutationFn: createInvocationTrigger,
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.triggers.all,
      });
      navigate('/invocation-triggers');
      toast.info('Invocation Trigger has been created');
    },
  });

  const handleSubmit = (data: any) => {
    const {
      scheduleConfig,
      triggerType,
      connectionId,
      sessionMode,
      ...restTrigger
    } = data;

    // Build configuration based on trigger type
    let configuration = null;
    if (triggerType === 'CRON' && scheduleConfig) {
      configuration = { expression: scheduleToCron(scheduleConfig) };
    } else if (triggerType === 'CHANNEL' && connectionId) {
      configuration = {
        connection_id: connectionId,
        ...(sessionMode && sessionMode !== 'per_sender'
          ? { session_mode: sessionMode }
          : {}),
      };
    }

    mutate({
      ...restTrigger,
      triggerType,
      configuration,
    });
  };

  if (isFetching) {
    return <Loader />;
  }

  return (
    <div className="w-full px-4 py-6 sm:px-6 lg:px-10">
      <div className="mx-auto flex w-full max-w-5xl flex-col gap-6">
        <section className="space-y-3 px-4 sm:px-5">
          <p className="text-xs font-semibold uppercase tracking-[0.08em] text-muted-foreground">
            Invocation triggers
          </p>
          <div className="space-y-2">
            <h1 className="text-3xl font-semibold leading-tight text-slate-900/90">
              Create trigger
            </h1>
            <p className="text-sm text-muted-foreground">
              Launch a scenario from an HTTP call, schedule, or application
              event.
            </p>
          </div>
        </section>

        <section className="space-y-4 px-4 sm:px-5">
          <TriggerForm
            title="Trigger details"
            description="Pick the scenario, trigger type, and any optional configuration."
            fieldProps={{ scenarios, connections }}
            isLoading={isPending}
            submitLabel="Create trigger"
            loadingLabel="Creating..."
            onSubmit={handleSubmit}
          />
        </section>
      </div>
    </div>
  );
}
