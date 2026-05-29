import { Link } from 'react-router';
import { PlusIcon } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { TriggersGrid } from '@/features/triggers/components/TriggersGrid';
import { useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { getInvocationTriggers } from '@/features/triggers/queries';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { TilesPage } from '@/shared/components/tiles-page';
import { Icons } from '@/shared/components/icons';

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

  const err = error as any;
  const isNetworkError =
    err?.message?.includes('fetch') ||
    err?.code === 'ERR_NETWORK' ||
    !err?.response;

  return (
    <TilesPage
      kicker="Invocation triggers"
      title="Manage event sources"
      action={
        <Link to="/invocation-triggers/create" className="w-full sm:w-auto">
          <Button
            className="w-full sm:w-auto sm:px-4"
            disabled={isError}
          >
            <PlusIcon className="mr-2 h-4 w-4" />
            New trigger
          </Button>
        </Link>
      }
    >
      {isFetching ? (
        <div className="rounded-lg border divide-y">
          {[...Array(6)].map((_, i) => (
            <div key={i} className="flex items-center gap-4 px-3 py-2.5">
              <div className="h-4 w-40 rounded bg-muted/60 animate-pulse" />
              <div className="h-4 w-16 rounded bg-muted/60 animate-pulse" />
              <div className="h-4 w-16 rounded bg-muted/60 animate-pulse" />
              <div className="ml-auto h-4 w-48 rounded bg-muted/60 animate-pulse" />
            </div>
          ))}
        </div>
      ) : isError ? (
        <div className="rounded-lg border bg-muted/20 px-6 py-10 text-center">
          <Icons.warning className="mx-auto mb-4 h-10 w-10 text-destructive" />
          <p className="text-base font-semibold text-foreground">
            {isNetworkError
              ? 'Unable to connect to backend'
              : 'An error occurred'}
          </p>
          <p className="mt-1 text-sm text-muted-foreground">
            {isNetworkError
              ? 'Please check that the backend service is running and try again.'
              : 'There was a problem loading triggers. Please try again.'}
          </p>
          {import.meta.env.DEV && error && (
            <div className="mt-4 max-w-md mx-auto rounded-lg bg-destructive/10 p-3 text-left">
              <p className="text-xs font-mono text-destructive break-words">
                {error.message || 'Unknown error'}
              </p>
            </div>
          )}
        </div>
      ) : (
        <TriggersGrid data={invocationTriggers as any} />
      )}
    </TilesPage>
  );
}
