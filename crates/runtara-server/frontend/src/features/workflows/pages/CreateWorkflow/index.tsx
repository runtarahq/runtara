import { useNavigate } from 'react-router';
import { toast } from 'sonner';
import { WorkflowForm } from '@/features/workflows/components/WorkflowForm';
import { useCustomMutation } from '@/shared/hooks/api';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { queryKeys } from '@/shared/queries/query-keys';
import { queryClient } from '@/main.tsx';

import { createWorkflow } from '@/features/workflows/queries';

export function CreateWorkflow() {
  const navigate = useNavigate();
  usePageTitle('Create Workflow');

  const { mutate, isPending } = useCustomMutation({
    mutationFn: createWorkflow,
    onSuccess: (response: any) => {
      // Response structure: { data: WorkflowDto, message: string, success: boolean }
      const workflowId = response?.data?.id;
      const message = response?.message;

      if (workflowId) {
        queryClient.invalidateQueries({ queryKey: queryKeys.workflows.all });
        navigate(`/workflows/${workflowId}`);
        toast.info(message || 'Workflow has been created');
      } else {
        console.error('No workflowId found in response:', response);
        toast.error('Workflow created but could not navigate to editor');
      }
    },
  });

  const handleSubmit = (data: Record<string, unknown>) => mutate(data as any);

  return (
    <div className="space-y-6">
      <WorkflowForm
        title="Create workflow"
        loading={isPending}
        onSubmit={handleSubmit}
      />
    </div>
  );
}
