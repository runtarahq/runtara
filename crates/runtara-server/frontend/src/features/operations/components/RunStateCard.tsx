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
import { StateValue, stateLabel, type StateField } from './StateValue';

export function StatePanel({
  state,
  schema = {},
  updatedAt,
}: {
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
    <Card>
      <CardHeader>
        <CardTitle className="text-base">State</CardTitle>
        {updatedAt && (
          <p className="text-xs text-muted-foreground">
            Updated{' '}
            <StateValue value={updatedAt} display={{ kind: 'relative' }} />
          </p>
        )}
      </CardHeader>
      <CardContent>
        {keys.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            No state published yet.
          </p>
        ) : (
          <dl className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
            {keys.map((key) => (
              <div key={key} className="min-w-0">
                <dt
                  className="mb-1 text-xs text-muted-foreground"
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
