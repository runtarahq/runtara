import { useQueryClient } from '@tanstack/react-query';
import { toast } from 'sonner';
import type { Instance } from '@/generated/RuntaraRuntimeApi';
import { useCustomMutation } from '@/shared/hooks/api';
import { updateInstance } from '@/features/objects/queries';
import { queryKeys } from '@/shared/queries/query-keys';
import { patchReportWritebackQueryData } from './reportWritebackCache';
import type { ReportWritebackSnapshot } from './reportWritebackCache';

type WritebackArgs = {
  schemaId: string;
  instanceId: string;
  field: string;
  value: unknown;
};

export function useReportWriteback(reportId: string) {
  const queryClient = useQueryClient();
  const reportQueryFilter = { queryKey: queryKeys.reports.byId(reportId) };

  return useCustomMutation<Instance, WritebackArgs>({
    mutationFn: (token, { schemaId, instanceId, field, value }) =>
      updateInstance(token, schemaId, instanceId, {
        properties: { [field]: value },
      }),
    onMutate: async (variables) => {
      await queryClient.cancelQueries(reportQueryFilter);

      const previousReportData = queryClient.getQueriesData(
        reportQueryFilter
      ) as ReportWritebackSnapshot;

      queryClient.setQueriesData(reportQueryFilter, (oldData) =>
        patchReportWritebackQueryData(oldData, variables)
      );

      return { previousReportData };
    },
    onSuccess: (instance, variables) => {
      queryClient.setQueriesData(reportQueryFilter, (oldData) =>
        patchReportWritebackQueryData(oldData, {
          ...variables,
          instance,
        })
      );
      toast.success('Updated');
    },
    onError: (_error, _variables, context) => {
      const previousReportData = (
        context as { previousReportData?: ReportWritebackSnapshot } | undefined
      )?.previousReportData;
      if (!previousReportData) return;

      for (const [queryKey, data] of previousReportData) {
        queryClient.setQueryData(queryKey, data);
      }
    },
  });
}
