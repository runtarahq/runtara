import { stateLabel } from '../state-label';
import { inlineOptions } from '../answer-options';
import { useRef, useState } from 'react';
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
import { StateValue, type StateField } from '../components/StateValue';
import { QueueAnswer, type AnswerController } from '../components/QueueAnswer';
import { ViewEditor, StateFilters } from '../components/ViewEditor';
import { OperationHeader, RunRows } from './shared';

const requestKey = (r: OperationRequest) =>
  JSON.stringify([r.instanceId, r.requestId]);
export function QueuePage() {
  const { workflowId, actionKey, viewId } = useParams();
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
        Could not load view. Refresh to try again.
      </p>
    );
  if (!initial)
    return (
      <div className="p-6">
        {queues.isPending || views.isPending || processes.isPending
          ? 'Loading view…'
          : 'View not found.'}
      </div>
    );
  return (
    <QueueContent
      key={viewId ?? `${workflowId}/${actionKey}`}
      initial={initial}
      schema={process?.stateSchema ?? queue?.stateSchema ?? {}}
      saved={saved}
    />
  );
}

function QueueContent({
  initial,
  schema,
  saved,
}: {
  initial: OperationViewConfig;
  schema: Record<string, StateField>;
  saved?: import('@/generated/RuntaraRuntimeApi').SavedOperationView;
}) {
  const [view, setView] = useState(initial);
  const [page, setPage] = useState(0);
  const [search, setSearch] = useState('');
  const [selection, setSelection] = useState<Set<string>>(() => new Set());
  const [editing, setEditing] = useState(false);
  const [filtersOpen, setFiltersOpen] = useState(false);
  const refs = useRef(new Map<string, AnswerController>());
  const tenant = useAuthStore((s) => s.orgId);
  const navigate = useNavigate();
  const submissions = useManagedInputSubmissions();
  const isQueue = Boolean(view.where?.openRequest);
  const columns = view.columns?.filter((field) => field !== view.roles?.key);
  const bulk = view.answers?.bulk && (!saved || Boolean(view.answers.inline));
  const query = useCustomQuery({
    queryKey: ['operations', tenant, 'items', view, page, search],
    queryFn: async (token: string) =>
      isQueue
        ? {
            requests: await queryRequests(token, view, page, search),
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
  function filters(state: StateFilterDto[]) {
    setView((v) => ({ ...v, where: { ...v.where, state } }));
    setPage(0);
    setSelection(new Set());
  }
  const goPage = (next: number) => {
    setPage(next);
    setSelection(new Set());
  };
  return (
    <ConsoleTableShell
      toolbar={
        <>
          <OperationHeader
            title={view.name}
            actions={
              <>
                <Button
                  variant="secondary"
                  onClick={() => void query.refetch()}
                >
                  Refresh
                </Button>
                <Can permission="workflow:update">
                  <Button
                    variant="secondary"
                    onClick={() => setEditing((v) => !v)}
                  >
                    {saved ? 'Edit view' : 'Save view'}
                  </Button>
                </Can>
              </>
            }
          />
          <div className="flex flex-wrap gap-3 border-b px-6 py-4">
            <Input
              aria-label="Search runs"
              placeholder="Search label or run"
              className="max-w-xs"
              value={search}
              onChange={(e) => {
                setSearch(e.target.value);
                goPage(0);
              }}
            />
            <Button
              variant="secondary"
              onClick={() => setFiltersOpen((v) => !v)}
            >
              Filters
              {view.where?.state?.length ? ` (${view.where.state.length})` : ''}
            </Button>
            {view.roles?.due ? (
              <Button
                variant="secondary"
                onClick={() =>
                  filters([
                    ...(view.where?.state ?? []),
                    {
                      field: view.roles!.due!,
                      op: 'lt',
                      value: { relative: 'now', offsetSeconds: 0 },
                    },
                  ])
                }
              >
                Overdue
              </Button>
            ) : null}
            <select
              aria-label="Sort by state"
              className="rounded border bg-background px-3 text-sm"
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
              <option value="">
                {isQueue ? 'Oldest request first' : 'Recent runs first'}
              </option>
              {Object.keys(schema).map((field) => (
                <option key={field} value={field}>
                  {stateLabel(field, schema[field])}
                </option>
              ))}
            </select>
            {view.sort ? (
              <Button
                variant="secondary"
                onClick={() => {
                  setView((v) => ({
                    ...v,
                    sort: { ...v.sort!, descending: !v.sort?.descending },
                  }));
                  goPage(0);
                }}
              >
                {view.sort.descending ? 'Descending' : 'Ascending'}
              </Button>
            ) : null}
          </div>
        </>
      }
      footer={
        <div className="flex items-center justify-between border-t px-6 py-3 text-xs text-muted-foreground">
          <span>
            {data?.totalElements ?? 0} {isQueue ? 'requests' : 'runs'}
            {query.isFetching ? ' · Updating…' : ''}
          </span>
          <TablePagination
            pageIndex={page}
            pageSize={25}
            pageCount={data?.totalPages ?? 0}
            onPageChange={goPage}
          />
        </div>
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
              navigate(`/operations/views/${result.id}`);
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
          <div className="m-6 flex flex-wrap items-center gap-3 rounded border bg-primary/5 p-3 text-sm">
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
          <thead className="sticky top-0 bg-muted/90 text-xs text-muted-foreground">
            <tr>
              <th className="w-12 p-4">
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
              <th className="p-4">
                {view.roles?.key
                  ? stateLabel(view.roles.key, schema[view.roles.key])
                  : 'Run'}
              </th>
              {columns?.map((field) => (
                <th key={field} className="p-4">
                  {view.labels?.[field] || stateLabel(field, schema[field])}
                </th>
              ))}
              <th className="p-4">Requested</th>
              <th className="p-4">Answer</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr
                key={requestKey(row)}
                className="border-b align-top hover:bg-muted/20"
              >
                <td className="p-4">
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
                <td className="max-w-64 p-4">
                  <Link
                    className="font-medium text-primary hover:underline"
                    to={`/operations/runs/${row.workflowId}/${row.instanceId}`}
                  >
                    {String(
                      (view.roles?.key && row.state?.[view.roles.key]) ??
                        row.runLabel ??
                        row.instanceId.slice(0, 8)
                    )}
                  </Link>
                  <p className="mt-1 text-xs text-muted-foreground">
                    {row.label}
                  </p>
                  {row.message ? <p className="mt-2">{row.message}</p> : null}
                  {row.context && Object.keys(row.context).length ? (
                    <details className="mt-2">
                      <summary>Context</summary>
                      <pre className="max-w-64 whitespace-pre-wrap text-xs">
                        {JSON.stringify(row.context, null, 2)}
                      </pre>
                    </details>
                  ) : null}
                </td>
                {columns?.map((field) => (
                  <td key={field} className="max-w-xs p-4">
                    <StateValue
                      value={row.state?.[field]}
                      field={schema[field]}
                      display={view.formats?.[field]}
                    />
                    {field === view.roles?.due &&
                    typeof row.state?.[field] === 'string' &&
                    Date.parse(row.state[field]) < Date.now() ? (
                      <span className="mt-1 block text-xs text-warning">
                        Overdue
                      </span>
                    ) : null}
                  </td>
                ))}
                <td className="whitespace-nowrap p-4">
                  <StateValue
                    value={row.requestedAt}
                    display={{ kind: 'relative' }}
                  />
                  {row.deadline ? (
                    <p className="mt-1 text-xs text-muted-foreground">
                      Deadline{' '}
                      <StateValue
                        value={row.deadline}
                        display={{ kind: 'relative' }}
                      />
                    </p>
                  ) : null}
                </td>
                <td className="p-4">
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
