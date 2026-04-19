import { useCallback, useState } from 'react';
import { toast } from 'sonner';
import { EnrichedTrigger } from '@/features/triggers/types';
import { useCustomMutation } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys.ts';
import { queryClient } from '@/main.tsx';
import { removeInvocationTrigger } from '@/features/triggers/queries';
import { TriggerCard } from '../TriggerCard';
import { Icons } from '@/shared/components/icons.tsx';

interface TriggersGridProps {
  data?: EnrichedTrigger[];
}

export function TriggersGrid({ data = [] }: TriggersGridProps) {
  const [deletingId, setDeletingId] = useState<string | null>(null);

  const removeMutation = useCustomMutation({
    mutationFn: removeInvocationTrigger,
    onSuccess: () => {
      toast.info('Invocation Trigger has been removed');
      queryClient.invalidateQueries({
        queryKey: queryKeys.triggers.all,
      });
    },
    onSettled: () => {
      setDeletingId(null);
    },
  });

  const handleDelete = useCallback(
    (id: string) => {
      setDeletingId(id);
      removeMutation.mutate(id);
    },
    [removeMutation]
  );

  if (!data || data.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center rounded-2xl bg-muted/20 px-6 py-12 text-center">
        <Icons.inbox className="mb-4 h-12 w-12 text-muted-foreground" />
        <p className="text-base font-semibold text-foreground">
          No triggers yet
        </p>
        <p className="mt-1 text-sm text-muted-foreground">
          Create your first trigger to connect external events.
        </p>
      </div>
    );
  }

  // Sort triggers by workflow name
  const sortedTriggers = [...data].sort((a, b) =>
    (a.workflowName || '').localeCompare(b.workflowName || '')
  );

  return (
    <div className="space-y-3">
      {sortedTriggers.map((trigger: EnrichedTrigger) => (
        <TriggerCard
          key={trigger.id}
          trigger={trigger}
          onDelete={handleDelete}
          loading={deletingId === trigger.id}
        />
      ))}
    </div>
  );
}
