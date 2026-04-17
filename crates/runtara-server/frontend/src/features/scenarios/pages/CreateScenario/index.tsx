import { useNavigate } from 'react-router';
import { toast } from 'sonner';
import { ScenarioForm } from '@/features/scenarios/components/ScenarioForm';
import { useCustomMutation } from '@/shared/hooks/api';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { queryKeys } from '@/shared/queries/query-keys';
import { queryClient } from '@/main.tsx';

import { createScenario } from '@/features/scenarios/queries';

export function CreateScenario() {
  const navigate = useNavigate();
  usePageTitle('Create Scenario');

  const { mutate, isPending } = useCustomMutation({
    mutationFn: createScenario,
    onSuccess: (response: any) => {
      // Response structure: { data: ScenarioDto, message: string, success: boolean }
      const scenarioId = response?.data?.id;
      const message = response?.message;

      if (scenarioId) {
        queryClient.invalidateQueries({ queryKey: queryKeys.scenarios.all });
        navigate(`/scenarios/${scenarioId}`);
        toast.info(message || 'Scenario has been created');
      } else {
        console.error('No scenarioId found in response:', response);
        toast.error('Scenario created but could not navigate to editor');
      }
    },
  });

  const handleSubmit = (data: Record<string, unknown>) => mutate(data as any);

  return (
    <div className="space-y-6">
      <ScenarioForm
        title="Create scenario"
        loading={isPending}
        onSubmit={handleSubmit}
      />
    </div>
  );
}
