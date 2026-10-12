import { stateLabel } from '../state-label';
import { inlineOptions } from '../answer-options';
import { useRef, useState } from 'react';
import {
  ArrowDown,
  ArrowDownWideNarrow,
  ArrowUp,
  ArrowUpNarrowWide,
  SlidersHorizontal,
} from 'lucide-react';
import { cn } from '@/lib/utils';
import { Link, useNavigate, useParams } from 'react-router';
import type {
  OperationRequest,
  OperationViewConfig,
  StateFilterDto,
} from '@/generated/RuntaraRuntimeApi';
import { useCustomQuery } from '@/shared/hooks/api';
import { useAuthStore } from '@/shared/stores/authStore';
import { Can } from '@/shared/components/Can';
import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import { WithTooltip } from '@/shared/components/ui/tooltip';
import {
  ConsoleTableShell,
  TablePagination,
} from '@/shared/components/console';
import { InputRetryPanel } from '@/features/workflows/components/ManagedInputSubmissions';
import { useManagedInputSubmissions } from '@/features/workflows/hooks/useManagedInputSubmissions';
import {
  defaultView,
  queryRequests,
  queryRuns,
  resolveQuery,
  useOperations,
} from '../queries';
import {
  DueValue,
  StateValue,
  type StateField,
} from '../components/StateValue';
import { QueueAnswer, type AnswerController } from '../components/QueueAnswer';
import { DeleteQueue } from '../components/DeleteQueue';
import { ViewEditor, StateFilters } from '../components/ViewEditor';
import { overdueFilters } from '../attention-requests';
import { OperationHeader, RunRows, RefreshControls } from './shared';

const requestKey = (r: OperationRequest) =>
  JSON.stringify([r.instanceId, r.requestId]);
const isNumeric = (field?: StateField) =>
  field?.type === 'number' || field?.type === 'integer';
export function QueuePage() {
  const { workflowId, actionKey, viewId: legacyViewId, queueId } = useParams();
  const viewId = queueId ?? legacyViewId;
  const { queues, views, processes } = useOperations();
  const saved = views.data?.find((v) => v.id === viewId);
  const queue = queues.data?.find(
    (q) => q.workflowId === workflowId && q.actionKey === actionKey
  );
  const process = processes.data?.find(
    (p) => p.workflowId === (saved?.configuration.workflow ?? workflowId)
  );
  const initial =
    saved?.configuration ??
    (queue
      ? defaultView(queue)
      : !actionKey && process
        ? {
            name: `${process.name} runs`,
            workflow: process.workflowId,
            columns: Object.keys(process.stateSchema ?? {}).slice(0, 29),
            where: {},
            roles: {},
            answers: {},
            formats: {},
          }
        : undefined);
  if (queues.error || views.error || processes.error)
    return (
      <p role="alert" className="p-6">
        Could not load queue. Refresh to try again.
      </p>
    );
  if (!initial)
    return (
      <div className="p-6">
        {queues.isPending || views.isPending || processes.isPending
          ? 'Loading queue…'
          : 'Queue not found.'}
      </div>
    );
  return (
    <QueueContent
      key={viewId ?? `${workflowId}/${actionKey}`}
      initial={initial}
      schema={process?.stateSchema ?? queue?.stateSchema ?? {}}
      saved={saved}
      workflowName={process?.name}
    />
  );
}

function QueueContent({
  initial,
  schema,
  saved,
  workflowName,
}: {
  initial: OperationViewConfig;
  schema: Record<string, StateField>;
  saved?: import('@/generated/RuntaraRuntimeApi').SavedOperationView;
  workflowName?: string;
}) {
  const [view, setView] = useState(initial);
  const [page, setPage] = useState(0);
  const [search, setSearch] = useState('');
  const [selection, setSelection] = useState<Set<string>>(() => new Set());
  const [editing, setEditing] = useState(false);
  const [filtersOpen, setFiltersOpen] = useState(false);
  const [tab, setTab] = useState<'open' | 'overdue'>('open');
  const refs = useRef(new Map<string, AnswerController>());
  const tenant = useAuthStore((s) => s.orgId);
  const navigate = useNavigate();
  const submissions = useManagedInputSubmissions();
  const isQueue = Boolean(view.where?.openRequest);
  const columns = view.columns?.filter((field) => field !== view.roles?.key);
  const bulk = view.answers?.bulk && (!saved || Boolean(view.answers.inline));
  const due = view.roles?.due;
  const overdueTab = isQueue && Boolean(due) && tab === 'overdue';
  const query = useCustomQuery({
    queryKey: ['operations', tenant, 'items', view, page, search, overdueTab],
    queryFn: async (token: string) =>
      isQueue
        ? {
            requests: await queryRequests(
              token,
              overdueTab ? withOverdue(view) : view,
              page,
              search
            ),
            runs: null,
          }
        : {
            requests: null,
            runs: await queryRuns(token, {
              ...resolveQuery(view),
              search,
              page,
              size: 25,
            }),
          },
    refetchInterval: 10_000,
    placeholderData: undefined,
  });
  // Tab counts, the same way the Overview counts overdue requests.
  const counts = useCustomQuery({
    queryKey: ['operations', tenant, 'queue-counts', view],
    queryFn: async (token: string) => {
      const [open, overdue] = await Promise.all([
        queryRequests(token, view, 0, '', 1),
        queryRequests(token, withOverdue(view), 0, '', 1),
      ]);
      return { open: open.totalElements, overdue: overdue.totalElements };
    },
    enabled: isQueue && Boolean(due),
    refetchInterval: 10_000,
    placeholderData: undefined,
  });
  const rows = query.data?.requests?.content ?? [];
  const selected = rows.filter(
    (row) =>
      selection.has(requestKey(row)) &&
      !submissions.inputs.some(
        (i) =>
          i.request.instanceId === row.instanceId &&
          i.request.requestId === row.requestId
      )
  );
  const options = selected.map((row) =>
    inlineOptions(row.inputSchema, view.answers?.inline, !saved)
  );
  const common =
    options.length && options.every(Boolean)
      ? options[0]!.values.filter((value) =>
          options.every((o) =>
            o!.values.some((v) => JSON.stringify(v) === JSON.stringify(value))
          )
        )
      : [];
  const data = query.data?.requests ?? query.data?.runs;
  const total = data?.totalElements;
  // Every row of a queue asks the same question; name it once in the header.
  const question = rows[0]?.label;
  // Context values that a column already shows are noise in the row.
  const shown = new Set([...(view.columns ?? []), view.roles?.key]);
  function filters(state: StateFilterDto[]) {
    setView((v) => ({ ...v, where: { ...v.where, state } }));
    setPage(0);
    setSelection(new Set());
  }
  const goPage = (next: number) => {
    setPage(next);
    setSelection(new Set());
  };
  // Header sorting cycles ascending, descending, then back to the default.
  const sortBy = (field: string) => {
    setView((v) => ({
      ...v,
      sort:
        v.sort?.field !== field
          ? { field, descending: false }
          : v.sort.descending
            ? null
            : { field, descending: true },
    }));
    goPage(0);
  };
  const tabs = [
    { key: 'open' as const, label: 'Open', count: counts.data?.open ?? total },
    ...(due
      ? [
          {
            key: 'overdue' as const,
            label: 'Overdue',
            count: counts.data?.overdue,
          },
        ]
      : []),
  ];
  return (
    <ConsoleTableShell
      className="mx-auto h-auto max-h-none min-h-full w-full max-w-[1600px] overflow-visible p-5 lg:px-10 lg:py-7"
      bodyClassName="flex-none rounded-lg border"
      toolbar={
        <>
          <OperationHeader
            title={view.name}
            section="Queues"
            description={
              workflowName ? (
                <Link
                  className="hover:text-foreground hover:underline"
                  to={`/workflows/${view.workflow}`}
                >
                  {workflowName}
                </Link>
              ) : isQueue ? (
                'Requests waiting for a decision.'
              ) : (
                'Follow runs and their published state.'
              )
            }
            actions={
              <>
                <RefreshControls
                  compact
                  updatedAt={query.dataUpdatedAt}
                  busy={query.isFetching}
                  onRefresh={() => {
                    void query.refetch();
                    void counts.refetch();
                  }}
                />
                <Can permission="workflow:update">
                  {saved ? (
                    <>
                      <Button asChild variant="secondary">
                        <Link to={`/operations/queues/${saved.id}/edit`}>
                          Edit queue
                        </Link>
                      </Button>
                      <DeleteQueue
                        queue={saved}
                        icon
                        onDeleted={() => navigate('/operations/queues')}
                      />
                    </>
                  ) : (
                    <Button
                      variant="secondary"
                      onClick={() => setEditing((value) => !value)}
                    >
                      Create queue
                    </Button>
                  )}
                </Can>
              </>
            }
          />
          <div className="mb-4 flex flex-wrap items-center gap-2">
            {isQueue ? (
              <div
                role="tablist"
                aria-label="Requests"
                className="mr-auto flex items-center gap-1 rounded-lg bg-muted p-1 text-sm"
              >
                {tabs.map((t) => {
                  const active = (overdueTab ? 'overdue' : 'open') === t.key;
                  return (
                    <button
                      key={t.key}
                      type="button"
                      role="tab"
                      aria-selected={active}
                      className={cn(
                        'rounded-md px-3 py-1 font-medium transition-colors',
                        active
                          ? 'bg-background text-foreground shadow-sm'
                          : 'text-muted-foreground hover:text-foreground'
                      )}
                      onClick={() => {
                        setTab(t.key);
                        goPage(0);
                      }}
                    >
                      {t.label}
                      <span
                        className={cn(
                          'ml-1.5 tabular-nums',
                          t.key === 'overdue' && t.count
                            ? 'text-warning'
                            : 'text-muted-foreground'
                        )}
                      >
                        {t.count?.toLocaleString() ?? '…'}
                      </span>
                    </button>
                  );
                })}
              </div>
            ) : (
              <p className="mr-auto text-sm font-medium tabular-nums">
                {total?.toLocaleString() ?? '…'}{' '}
                <span className="font-normal text-muted-foreground">
                  {total === 1 ? 'run' : 'runs'}
                </span>
              </p>
            )}
            <Input
              aria-label="Search runs"
              placeholder="Search label or run"
              className="w-full sm:w-64"
              value={search}
              onChange={(e) => {
                setSearch(e.target.value);
                goPage(0);
              }}
            />
            <Button
              variant="secondary"
              aria-expanded={filtersOpen}
              onClick={() => setFiltersOpen((v) => !v)}
            >
              <SlidersHorizontal />
              Filters
              {view.where?.state?.length ? ` (${view.where.state.length})` : ''}
            </Button>
            {isQueue ? null : (
              <div className="flex items-center gap-1">
                <select
                  aria-label="Sort by"
                  className="h-8 max-w-full rounded-md border bg-background px-2 text-sm"
                  value={view.sort?.field ?? ''}
                  onChange={(e) => {
                    setView((v) => ({
                      ...v,
                      sort: e.target.value
                        ? { field: e.target.value, descending: false }
                        : null,
                    }));
                    goPage(0);
                  }}
                >
                  <option value="">Recent runs first</option>
                  {Object.keys(schema).map((field) => (
                    <option key={field} value={field}>
                      Sort by {stateLabel(field, schema[field])}
                    </option>
                  ))}
                </select>
                {view.sort ? (
                  <WithTooltip
                    label={
                      view.sort.descending
                        ? 'Descending — switch to ascending'
                        : 'Ascending — switch to descending'
                    }
                  >
                    <Button
                      variant="secondary"
                      bordered
                      size="icon"
                      aria-label={
                        view.sort.descending
                          ? 'Sort descending'
                          : 'Sort ascending'
                      }
                      onClick={() => sortBy(view.sort!.field)}
                    >
                      {view.sort.descending ? (
                        <ArrowDownWideNarrow />
                      ) : (
                        <ArrowUpNarrowWide />
                      )}
                    </Button>
                  </WithTooltip>
                ) : null}
              </div>
            )}
          </div>
        </>
      }
      footer={
        (data?.totalPages ?? 0) > 1 ? (
          <div className="flex justify-end py-3">
            <TablePagination
              pageIndex={page}
              pageSize={25}
              pageCount={data?.totalPages ?? 0}
              onPageChange={goPage}
            />
          </div>
        ) : undefined
      }
    >
      {editing ? (
        <Can permission="workflow:update">
          <ViewEditor
            initial={view}
            saved={saved}
            schema={schema}
            onCancel={() => setEditing(false)}
            onSaved={(result) => {
              setEditing(false);
              setView(result.configuration);
              navigate(`/operations/queues/${result.id}`);
            }}
          />
        </Can>
      ) : null}
      {filtersOpen ? (
        <div className="border-b p-6">
          <StateFilters
            filters={view.where?.state ?? []}
            fields={Object.keys(schema)}
            schema={schema}
            onChange={filters}
          />
        </div>
      ) : null}
      <Can permission="workflow:execute">
        <div className="px-6">
          <InputRetryPanel
            matches={(request) => request.workflowId === view.workflow}
          />
        </div>
      </Can>
      {isQueue && bulk && selected.length ? (
        <Can permission="workflow:execute">
          <div className="flex flex-wrap items-center gap-3 border-b bg-primary/5 px-4 py-3 text-sm">
            <strong>{selected.length} requests selected</strong>
            {common.map((value) => (
              <Button
                size="sm"
                key={JSON.stringify(value)}
                onClick={() =>
                  void Promise.all(
                    selected.map((row) =>
                      refs.current.get(requestKey(row))?.choose(value)
                    )
                  )
                }
              >
                {stateLabel(String(value))} selected
              </Button>
            ))}
            <Button
              size="sm"
              variant="secondary"
              bordered
              onClick={() =>
                void Promise.all(
                  selected.map((row) =>
                    refs.current.get(requestKey(row))?.submit()
                  )
                )
              }
            >
              Submit prepared answers
            </Button>
            <Button
              size="sm"
              variant="secondary"
              bordered
              onClick={() => setSelection(new Set())}
            >
              Clear
            </Button>
            <p className="basis-full text-muted-foreground">
              Additional fields appear in each row. Review them before
              submitting.
            </p>
          </div>
        </Can>
      ) : null}
      {query.error ? (
        <p role="alert" className="p-6 text-destructive">
          Could not load this view. Check its filters or refresh to try again.
        </p>
      ) : query.isPending ? (
        <p className="p-6">Loading…</p>
      ) : isQueue ? (
        <table className="w-full border-collapse text-left text-sm">
          <thead className="sticky top-0 bg-muted/90 text-[11px] uppercase tracking-wide text-muted-foreground">
            <tr>
              <th className="w-12 px-4 py-3">
                <Can permission="workflow:execute">
                  {bulk ? (
                    <input
                      aria-label="Select page"
                      type="checkbox"
                      checked={
                        rows.length > 0 &&
                        rows.every((row) => selection.has(requestKey(row)))
                      }
                      onChange={(e) =>
                        setSelection(
                          e.target.checked
                            ? new Set(rows.map(requestKey))
                            : new Set()
                        )
                      }
                    />
                  ) : null}
                </Can>
              </th>
              {view.roles?.key ? (
                <SortHeader
                  field={view.roles.key}
                  label={stateLabel(view.roles.key, schema[view.roles.key])}
                  sort={view.sort}
                  onSort={sortBy}
                />
              ) : (
                <th className="px-4 py-3">Run</th>
              )}
              {columns?.map((field) => (
                <SortHeader
                  key={field}
                  field={field}
                  label={
                    view.labels?.[field] || stateLabel(field, schema[field])
                  }
                  numeric={isNumeric(schema[field])}
                  sort={view.sort}
                  onSort={sortBy}
                />
              ))}
              <th className="px-4 py-3">{question ?? 'Decision'}</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr
                key={requestKey(row)}
                className={`border-b last:border-0 ${selection.has(requestKey(row)) ? 'bg-primary/5' : 'hover:bg-muted/20'}`}
              >
                <td className="px-4 py-3">
                  <Can permission="workflow:execute">
                    {bulk ? (
                      <input
                        type="checkbox"
                        aria-label={`Select ${row.runLabel ?? row.label} ${row.requestId}`}
                        checked={selection.has(requestKey(row))}
                        onChange={(e) =>
                          setSelection((previous) => {
                            const next = new Set(previous);
                            if (e.target.checked) next.add(requestKey(row));
                            else next.delete(requestKey(row));
                            return next;
                          })
                        }
                      />
                    ) : null}
                  </Can>
                </td>
                <td className="max-w-64 px-4 py-3">
                  <Link
                    className="whitespace-nowrap font-medium text-primary-text hover:underline"
                    to={`/operations/runs/${row.workflowId}/${row.instanceId}`}
                  >
                    {String(
                      (view.roles?.key && row.state?.[view.roles.key]) ??
                        row.runLabel ??
                        row.instanceId.slice(0, 8)
                    )}
                  </Link>
                  <p className="mt-0.5 text-xs text-muted-foreground">
                    Requested{' '}
                    <StateValue
                      value={row.requestedAt}
                      display={{ kind: 'relative' }}
                    />
                    {row.deadline ? (
                      <>
                        {' · deadline '}
                        <StateValue
                          value={row.deadline}
                          display={{ kind: 'relative' }}
                        />
                      </>
                    ) : null}
                  </p>
                  {row.message && row.message !== row.label ? (
                    <p className="mt-1 text-xs">{row.message}</p>
                  ) : null}
                  <RequestContext
                    context={row.context}
                    hide={shown}
                    schema={schema}
                    formats={view.formats}
                  />
                </td>
                {columns?.map((field) => (
                  <td
                    key={field}
                    className={`max-w-xs px-4 py-3 ${isNumeric(schema[field]) ? 'text-right tabular-nums' : ''}`}
                  >
                    {field === due ? (
                      <DueValue value={row.state?.[field]} />
                    ) : (
                      <StateValue
                        value={row.state?.[field]}
                        field={schema[field]}
                        display={view.formats?.[field]}
                      />
                    )}
                  </td>
                ))}
                <td className="px-4 py-3">
                  <Can
                    permission="workflow:execute"
                    fallback={
                      <span className="text-muted-foreground">Read only</span>
                    }
                  >
                    <QueueAnswer
                      row={row}
                      inline={view.answers?.inline}
                      infer={!saved}
                      ref={(controller) => {
                        if (controller)
                          refs.current.set(requestKey(row), controller);
                        else refs.current.delete(requestKey(row));
                      }}
                    />
                  </Can>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : (
        <RunRows
          rows={query.data?.runs?.content ?? []}
          view={view}
          schema={schema}
        />
      )}
      {!query.isPending && !query.error && (data?.totalElements ?? 0) === 0 ? (
        <p className="p-10 text-center text-muted-foreground">
          {isQueue
            ? 'No requests waiting for an answer.'
            : 'No runs match this view.'}
        </p>
      ) : null}
    </ConsoleTableShell>
  );
}

/** Context the request carries, as label/value pairs, minus what columns show. */
function RequestContext({
  context,
  hide,
  schema,
  formats,
}: {
  context: unknown;
  hide: Set<string | null | undefined>;
  schema: Record<string, StateField>;
  formats?: OperationViewConfig['formats'];
}) {
  if (!context || typeof context !== 'object' || Array.isArray(context))
    return null;
  const entries = Object.entries(context).filter(([key]) => !hide.has(key));
  if (!entries.length) return null;
  return (
    <dl className="mt-1 space-y-0.5 text-xs">
      {entries.map(([key, value]) => (
        <div key={key} className="flex min-w-0 gap-1">
          <dt className="shrink-0 text-muted-foreground">
            {stateLabel(key, schema[key])}:
          </dt>
          <dd className="min-w-0 truncate">
            <StateValue
              value={value}
              field={schema[key]}
              display={formats?.[key] ?? undefined}
            />
          </dd>
        </div>
      ))}
    </dl>
  );
}

/** Overdue is a lens on the queue, applied per query and never saved. */
function withOverdue(view: OperationViewConfig): OperationViewConfig {
  return {
    ...view,
    where: {
      ...view.where,
      state: [
        ...(view.where?.state ?? []),
        ...((overdueFilters(view, Date.now()) ?? []) as StateFilterDto[]),
      ],
    },
  };
}

function SortHeader({
  field,
  label,
  numeric,
  sort,
  onSort,
}: {
  field: string;
  label: string;
  numeric?: boolean;
  sort?: OperationViewConfig['sort'];
  onSort: (field: string) => void;
}) {
  const active = sort?.field === field;
  const Arrow = sort?.descending ? ArrowDown : ArrowUp;
  return (
    <th
      aria-sort={
        active ? (sort?.descending ? 'descending' : 'ascending') : undefined
      }
      className={cn('px-4 py-3', numeric && 'text-right')}
    >
      <button
        type="button"
        className={cn(
          'inline-flex items-center gap-1 uppercase tracking-wide hover:text-foreground',
          active && 'text-foreground'
        )}
        onClick={() => onSort(field)}
      >
        {label}
        {active ? <Arrow className="size-3" /> : null}
      </button>
    </th>
  );
}
