import { stateLabel } from '../state-label';
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from '@/shared/components/ui/card';
import { useCustomQuery } from '@/shared/hooks/api';
import { RuntimeREST } from '@/shared/queries';
import { createAuthHeaders } from '@/shared/queries/utils';
import { queryKeys } from '@/shared/queries/query-keys';
import { StateValue, type StateField } from './StateValue';

export function StatePanel({
  state,
  schema = {},
  updatedAt,
  compact = false,
}: {
  compact?: boolean;
  state?: Record<string, unknown> | null;
  schema?: Record<string, StateField>;
  updatedAt?: string | null;
}) {
  const keys = [
    ...new Set([...Object.keys(schema), ...Object.keys(state ?? {})]),
  ].sort(
    (a, b) =>
      (schema[a]?.order ?? 0) - (schema[b]?.order ?? 0) || a.localeCompare(b)
  );
  return (
    <Card
      className={compact ? 'gap-0 overflow-hidden py-0 shadow-none' : undefined}
    >
      <CardHeader
        className={
          compact
            ? 'flex flex-row flex-wrap items-center justify-between gap-2 border-b px-4 py-3'
            : undefined
        }
      >
        <CardTitle className="text-base">
          <h2>State</h2>
        </CardTitle>
        {updatedAt && (
          <p className="text-xs text-muted-foreground">
            Updated{' '}
            <StateValue value={updatedAt} display={{ kind: 'relative' }} />
          </p>
        )}
      </CardHeader>
      <CardContent className={compact ? 'px-4 py-1' : undefined}>
        {keys.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            No state published yet.
          </p>
        ) : (
          <dl
            className={
              compact ? 'divide-y' : 'grid gap-4 sm:grid-cols-2 lg:grid-cols-3'
            }
          >
            {keys.map((key) => (
              <div
                key={key}
                className={
                  compact
                    ? 'grid min-w-0 grid-cols-[minmax(0,2fr)_minmax(0,3fr)] gap-4 py-2.5'
                    : 'min-w-0'
                }
              >
                <dt
                  className={
                    compact
                      ? 'text-xs text-muted-foreground'
                      : 'mb-1 text-xs text-muted-foreground'
                  }
                  title={schema[key]?.description}
                >
                  {stateLabel(key, schema[key])}
                </dt>
                <dd className="text-sm">
                  <StateValue value={state?.[key]} field={schema[key]} />
                </dd>
              </div>
            ))}
          </dl>
        )}
      </CardContent>
    </Card>
  );
}

export function RunStateCard({
  workflowId,
  version,
  state,
  updatedAt,
}: {
  workflowId: string;
  version: number;
  state?: Record<string, unknown> | null;
  updatedAt?: string | null;
}) {
  const { data, error } = useCustomQuery({
    queryKey: queryKeys.workflows.schemas(workflowId, version),
    queryFn: async (token: string) =>
      (
        await RuntimeREST.api.getVersionSchemasHandler(
          workflowId,
          version,
          createAuthHeaders(token)
        )
      ).data,
    enabled: Boolean(workflowId) && version > 0,
    placeholderData: undefined,
  });
  return (
    <div className="space-y-2">
      <StatePanel
        state={state}
        schema={data?.stateSchema ?? {}}
        updatedAt={updatedAt}
      />
      {error && (
        <p role="status" className="text-xs text-muted-foreground">
          Field labels are unavailable. Showing stored values.
        </p>
      )}
    </div>
  );
}
