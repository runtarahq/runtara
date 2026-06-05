import { Link } from 'react-router';
import { PlusIcon } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { Can } from '@/shared/components/Can';
import { TriggersGrid } from '@/features/triggers/components/TriggersGrid';
import { useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { getInvocationTriggers } from '@/features/triggers/queries';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { Breadcrumb, ConsoleToolbar } from '@/shared/components/console';

export function Triggers() {
  const {
    data: invocationTriggers,
    isFetching,
    isError,
    error,
  } = useCustomQuery({
    queryKey: queryKeys.triggers.all,
    queryFn: getInvocationTriggers,
  });

  usePageTitle('Invocation Triggers');

  const toolbar = (
    <ConsoleToolbar
      left={<Breadcrumb items={[{ label: 'Triggers' }]} />}
      actions={
        <Can permission="trigger:create">
          <Link to="/invocation-triggers/create">
            <Button disabled={isError}>
              <PlusIcon className="mr-2 h-4 w-4" />
              New trigger
            </Button>
          </Link>
        </Can>
      }
    />
  );

  return (
    <TriggersGrid
      toolbar={toolbar}
      data={invocationTriggers as any}
      isFetching={isFetching}
      isError={isError}
      error={error}
    />
  );
}
