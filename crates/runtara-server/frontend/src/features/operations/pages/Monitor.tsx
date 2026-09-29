import { useState } from 'react';
import { Link } from 'react-router';
import { useQueryClient } from '@tanstack/react-query';
import type { OperationProcess } from '@/generated/RuntaraRuntimeApi';
import { useCustomQuery } from '@/shared/hooks/api';
import { useAuthStore } from '@/shared/stores/authStore';
import { TablePagination } from '@/shared/components/console';
import { StateValue } from '../components/StateValue';
import { queryRuns, useOperations } from '../queries';
import {
  OperationHeader,
  OperationSection,
  FailureRows,
  RefreshControls,
} from './shared';

export function MonitorPage() {
  const tenant = useAuthStore((s) => s.orgId);
  const client = useQueryClient();
  const { processes } = useOperations();
  const [page, setPage] = useState(0);
  const [hours, setHours] = useState(24);
  const [refresh, setRefresh] = useState(true);
  const interval = refresh ? 30_000 : false;
  const failures = useCustomQuery({
    queryKey: ['operations', tenant, 'monitor', 'failures', page, hours],
    queryFn: (token: string) =>
      queryRuns(token, {
        status: 'failed,timeout',
        completedFrom: hours
          ? new Date(Date.now() - hours * 3_600_000).toISOString()
          : undefined,
        sortBy: 'completedAt',
        sortOrder: 'desc',
        page,
        size: 10,
      }),
    refetchInterval: interval,
    placeholderData: undefined,
  });
  const waiting = useCustomQuery({
    queryKey: ['operations', tenant, 'monitor', 'waiting'],
    queryFn: (token: string) =>
      queryRuns(token, {
        status: 'suspended',
        sortBy: 'createdAt',
        sortOrder: 'asc',
        size: 5,
      }),
    refetchInterval: interval,
    placeholderData: undefined,
  });
  const period =
    hours === 24 ? 'last 24 hours' : hours === 168 ? 'last 7 days' : 'all time';
  return (
    <div className="mx-auto min-h-full w-full max-w-[1600px] bg-background p-5 lg:px-10 lg:py-7">
      <OperationHeader
        title="Monitor"
        section="Monitor"
        description="How each process is running, and which runs need attention."
        actions={
          <>
            <RefreshControls
              updatedAt={failures.dataUpdatedAt}
              busy={failures.isFetching}
              onRefresh={() =>
                void client.invalidateQueries({
                  queryKey: ['operations', tenant, 'monitor'],
                })
              }
            />
            <label className="flex items-center gap-2 whitespace-nowrap text-xs">
              <input
                type="checkbox"
                role="switch"
                checked={refresh}
                onChange={(e) => setRefresh(e.target.checked)}
              />
              Refresh every 30 s
            </label>
            <select
              aria-label="Monitor time range"
              className="h-8 rounded-md border bg-background px-3 text-sm"
              value={hours}
              onChange={(e) => {
                setHours(Number(e.target.value));
                setPage(0);
              }}
            >
              <option value={24}>Last 24 hours</option>
              <option value={168}>Last 7 days</option>
              <option value={0}>All time</option>
            </select>
          </>
        }
      />
      <main className="space-y-5">
        <OperationSection
          title="Processes"
          aside={
            hours ? `Runs started in the ${period}` : 'Runs started at any time'
          }
        >
          <div className="overflow-x-auto">
            <table
              className="w-full text-left text-sm"
              aria-label="Process health"
            >
              <thead className="bg-muted/50 text-[11px] uppercase tracking-wide text-muted-foreground">
                <tr>
                  <th className="min-w-52 px-4 py-3">Workflow</th>
                  {['Running', 'Waiting', 'Completed', 'Failed'].map(
                    (label) => (
                      <th key={label} className="px-4 py-3 text-right">
                        {label}
                      </th>
                    )
                  )}
                  <th className="whitespace-nowrap px-4 py-3 text-right">
                    Oldest waiting run
                  </th>
                  <th className="w-20 px-4 py-3">
                    <span className="sr-only">Action</span>
                  </th>
                </tr>
              </thead>
              <tbody className="divide-y">
                {processes.data?.map((process) => (
                  <ProcessRow
                    key={process.workflowId}
                    process={process}
                    hours={hours}
                    interval={interval}
                  />
                ))}
              </tbody>
            </table>
          </div>
          {processes.isPending ? (
            <p className="p-4 text-sm text-muted-foreground">
              Loading processes…
            </p>
          ) : processes.error ? (
            <p role="alert" className="p-4 text-sm">
              Could not load processes.
            </p>
          ) : !processes.data?.length ? (
            <p className="p-4 text-sm text-muted-foreground">
              No workflows yet.
            </p>
          ) : null}
        </OperationSection>
        <OperationSection
          title="Failed runs"
          aside={
            failures.isPending
              ? 'Loading…'
              : `${failures.data?.totalElements ?? '—'} ${hours ? `in the ${period}` : 'in all time'}`
          }
        >
          {failures.error ? (
            <p role="alert" className="p-4 text-sm">
              Could not refresh failed runs. Try Refresh.
            </p>
          ) : null}
          {failures.isPending ? (
            <p className="p-4 text-sm text-muted-foreground">
              Loading failures…
            </p>
          ) : (
            <FailureRows
              rows={(failures.data?.content ?? []).map((run) => ({
                ...run,
                workflowName:
                  processes.data?.find((p) => p.workflowId === run.workflowId)
                    ?.name ?? run.workflowName,
              }))}
            />
          )}
          {failures.data?.totalElements === 0 ? (
            <p className="p-5 text-sm text-muted-foreground">
              No failed runs in this period.
            </p>
          ) : null}
          {(failures.data?.totalPages ?? 0) > 1 ? (
            <div className="flex justify-end border-t p-3">
              <TablePagination
                pageIndex={page}
                pageSize={10}
                pageCount={failures.data?.totalPages ?? 0}
                onPageChange={setPage}
              />
            </div>
          ) : null}
        </OperationSection>
        <OperationSection
          title="Waiting longest"
          aside="Ordered by run start time"
        >
          {waiting.error ? (
            <p role="alert" className="p-4 text-sm">
              Could not refresh waiting runs.
            </p>
          ) : null}
          {waiting.isPending ? (
            <p className="p-4 text-sm text-muted-foreground">
              Loading waiting runs…
            </p>
          ) : (
            <ul className="divide-y">
              {waiting.data?.content.map((run) => (
                <li
                  key={run.id}
                  className="flex items-center justify-between gap-4 px-4 py-3"
                >
                  <div className="min-w-0">
                    <Link
                      className="break-words text-sm font-semibold hover:text-primary-text"
                      to={`/operations/runs/${run.workflowId}/${run.id}`}
                    >
                      {run.runLabel ?? run.id.slice(0, 8)}
                    </Link>
                    <p className="mt-0.5 text-xs text-muted-foreground">
                      {processes.data?.find(
                        (p) => p.workflowId === run.workflowId
                      )?.name ?? run.workflowName}{' '}
                      · {run.suspensionReason?.replace(/_/g, ' ') ?? 'Waiting'}
                    </p>
                  </div>
                  <span
                    className="shrink-0 whitespace-nowrap text-xs text-muted-foreground"
                    title="Elapsed since the run started"
                  >
                    Started{' '}
                    <StateValue
                      value={run.created}
                      display={{ kind: 'relative' }}
                    />
                  </span>
                </li>
              ))}
            </ul>
          )}
          {waiting.data?.totalElements === 0 ? (
            <p className="p-5 text-sm text-muted-foreground">
              No waiting runs.
            </p>
          ) : null}
        </OperationSection>
      </main>
    </div>
  );
}
function ProcessRow({
  process,
  hours,
  interval,
}: {
  process: OperationProcess;
  hours: number;
  interval: number | false;
}) {
  const tenant = useAuthStore((s) => s.orgId);
  const counts = useCustomQuery({
    queryKey: [
      'operations',
      tenant,
      'monitor',
      'process',
      process.workflowId,
      hours,
    ],
    queryFn: async (token: string) => {
      const createdFrom = hours
        ? new Date(Date.now() - hours * 3_600_000).toISOString()
        : undefined;
      const pages = await Promise.all(
        ['running', 'suspended', 'completed', 'failed,timeout'].map((status) =>
          queryRuns(token, {
            workflowId: process.workflowId,
            status,
            createdFrom,
            size: 1,
            sortBy: 'createdAt',
            sortOrder: 'asc',
          })
        )
      );
      return {
        counts: pages.map((p) => p.totalElements),
        oldest: pages[1].content[0]?.created,
      };
    },
    refetchInterval: interval,
    placeholderData: undefined,
  });
  return (
    <tr>
      <td className="px-4 py-3">
        <Link
          className="font-medium text-primary-text"
          to={`/operations/processes/${process.workflowId}`}
        >
          {process.name}
        </Link>
        {counts.error ? (
          <p role="status" className="mt-1 text-xs text-destructive">
            Counts unavailable
          </p>
        ) : null}
      </td>
      {[0, 1, 2, 3].map((i) => (
        <td
          key={i}
          className={`px-4 py-3 text-right tabular-nums ${i === 3 && counts.data?.counts[i] ? 'font-medium text-destructive' : i === 1 && counts.data?.counts[i] ? 'font-medium text-warning' : ''}`}
        >
          {counts.data?.counts[i]?.toLocaleString() ?? '—'}
        </td>
      ))}
      <td
        className="whitespace-nowrap px-4 py-3 text-right text-xs"
        title="Elapsed since the oldest waiting run started"
      >
        {counts.data?.oldest ? (
          <StateValue
            value={counts.data.oldest}
            display={{ kind: 'relative' }}
          />
        ) : (
          '—'
        )}
      </td>
      <td className="px-4 py-3 text-right">
        <Link
          className="font-medium text-primary-text"
          to={`/operations/processes/${process.workflowId}`}
        >
          Open
        </Link>
      </td>
    </tr>
  );
}
