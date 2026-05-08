import { useQueryClient } from '@tanstack/react-query';
import { toast } from 'sonner';
import { useCustomMutation } from '@/shared/hooks/api';
import { updateInstance } from '@/features/objects/queries';
import { queryKeys } from '@/shared/queries/query-keys';

type WritebackArgs = {
  schemaId: string;
  instanceId: string;
  field: string;
  value: unknown;
};

export function useReportWriteback(reportId: string) {
  const queryClient = useQueryClient();

  return useCustomMutation<unknown, WritebackArgs>({
    mutationFn: (token, { schemaId, instanceId, field, value }) =>
      updateInstance(token, schemaId, instanceId, {
        properties: { [field]: value },
      }),
    onSuccess: () => {
      void queryClient.invalidateQueries({
        queryKey: queryKeys.reports.byId(reportId),
      });
      toast.success('Updated');
    },
  });
}
