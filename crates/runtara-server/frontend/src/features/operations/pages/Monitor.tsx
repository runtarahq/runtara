import { useState } from 'react';
import { useCustomQuery } from '@/shared/hooks/api';
import { useAuthStore } from '@/shared/stores/authStore';
import { Button } from '@/shared/components/ui/button';
import { TablePagination } from '@/shared/components/console';
import { queryRuns } from '../queries';
import { OperationHeader, RunRows } from './shared';
export function MonitorPage() {
  const tenant = useAuthStore((s) => s.orgId);
  const [page, setPage] = useState(0);
  const failures = useCustomQuery({
    queryKey: ['operations', tenant, 'failures', page],
    queryFn: (token: string) =>
      queryRuns(token, { status: 'failed,timeout', page, size: 25 }),
    refetchInterval: 10_000,
    placeholderData: undefined,
  });
  const waiting = useCustomQuery({
    queryKey: ['operations', tenant, 'waiting'],
    queryFn: (token: string) =>
      queryRuns(token, {
        status: 'suspended',
        sortBy: 'createdAt',
        sortOrder: 'asc',
        size: 25,
      }),
    refetchInterval: 10_000,
    placeholderData: undefined,
  });
  return (
    <div>
      <OperationHeader
        title="Monitor"
        actions={
          <Button
            variant="secondary"
            onClick={() => {
              void failures.refetch();
              void waiting.refetch();
            }}
          >
            Refresh
          </Button>
        }
      />
      <section className="py-6">
        <h2 className="px-6 text-lg font-semibold">Failures</h2>
        <p className="mb-4 px-6 text-sm text-muted-foreground">
          Review the error. Replay is available for transient failures and
          starts a new run.
        </p>
        {failures.error ? (
          <p role="alert" className="p-6">
            Could not load failures.
          </p>
        ) : failures.isPending ? (
          <p className="p-6">Loading…</p>
        ) : (
          <>
            <RunRows rows={failures.data?.content ?? []} failures />
            {failures.data?.totalElements === 0 ? (
              <p className="p-6 text-muted-foreground">No failed runs.</p>
            ) : null}
            <div className="flex justify-end p-4">
              <TablePagination
                pageIndex={page}
                pageSize={25}
                pageCount={failures.data?.totalPages ?? 0}
                onPageChange={setPage}
              />
            </div>
          </>
        )}
      </section>
      <section className="py-6">
        <h2 className="mb-3 px-6 text-lg font-semibold">
          Oldest suspended runs
        </h2>
        {waiting.error ? (
          <p role="alert" className="p-6">
            Could not load suspended runs.
          </p>
        ) : (
          <RunRows rows={waiting.data?.content ?? []} />
        )}
        {waiting.data?.totalElements === 0 ? (
          <p className="p-6 text-muted-foreground">No suspended runs.</p>
        ) : null}
      </section>
    </div>
  );
}
