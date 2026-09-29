import { stateLabel } from '../state-label';
import { useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import type {
  OperationViewConfig,
  OperationQueue,
  SavedOperationView,
  StateFilterDto,
} from '@/generated/RuntaraRuntimeApi';
import { useToken } from '@/shared/hooks';
import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import { operationsRequest, message } from '../queries';
import { type StateField } from './StateValue';

export function ViewEditor({
  initial,
  saved,
  schema,
  onSaved,
  onCancel,
  requestSources,
}: {
  requestSources?: OperationQueue[];
  initial: OperationViewConfig;
  saved?: SavedOperationView;
  schema: Record<string, StateField>;
  onSaved: (view: SavedOperationView) => void;
  onCancel: () => void;
}) {
  const [view, setView] = useState(initial);
  // Save against the revision this form was opened with, even if discovery refreshes.
  const [revision] = useState(saved?.revision);
  const [error, setError] = useState('');
  const [saving, setSaving] = useState(false);
  const token = useToken();
  const client = useQueryClient();
  const fields = [
    ...new Set([...Object.keys(schema), ...(view.columns ?? [])]),
  ];
  const update = (change: Partial<OperationViewConfig>) =>
    setView((v) => ({ ...v, ...change }));
  async function save() {
    setSaving(true);
    setError('');
    try {
      const result = await operationsRequest<SavedOperationView>(
        token,
        `operations/views${saved ? `/${saved.id}` : ''}`,
        saved ? 'PUT' : 'POST',
        { configuration: view, revision }
      );
      await client.invalidateQueries({ queryKey: ['operations'] });
      onSaved(result);
    } catch (e) {
      setError(message(e));
    } finally {
      setSaving(false);
    }
  }
  return (
    <section
      aria-label="Queue settings"
      className="m-6 space-y-5 rounded-lg border bg-muted/20 p-5"
    >
      <h2 className="text-lg font-semibold">Queue settings</h2>
      <label className="block max-w-lg text-sm">
        Name
        <Input
          value={view.name}
          onChange={(e) => update({ name: e.target.value })}
        />
      </label>
      {requestSources && (
        <label className="block max-w-lg text-sm">
          Queue contents
          <select
            aria-label="Queue contents"
            className="mt-1 block h-9 w-full rounded border bg-background px-3"
            value={view.where?.openRequest ?? ''}
            onChange={(event) =>
              update({
                where: {
                  ...view.where,
                  openRequest: event.target.value || null,
                  status: undefined,
                },
                answers: { ...view.answers, inline: null, bulk: false },
              })
            }
          >
            <option value="">Workflow runs</option>
            {view.where?.openRequest &&
              !requestSources.some(
                (source) => source.actionKey === view.where?.openRequest
              ) && (
                <option value={view.where.openRequest}>
                  {view.where.openRequest} (older requests)
                </option>
              )}
            {requestSources.map((source) => (
              <option key={source.actionKey} value={source.actionKey}>
                Requests: {source.name} ({source.actionKey})
              </option>
            ))}
          </select>
        </label>
      )}
      {!view.where?.openRequest && (
        <label className="block max-w-lg text-sm">
          Run status
          <Input
            placeholder="All statuses"
            value={view.where?.status ?? ''}
            onChange={(event) =>
              update({
                where: { ...view.where, status: event.target.value || null },
              })
            }
          />
          <span className="text-xs text-muted-foreground">
            For example: running, suspended, failed, completed. Separate
            multiple statuses with commas.
          </span>
        </label>
      )}
      <div className="flex flex-wrap items-end gap-4">
        <label className="text-sm">
          Sort field
          <select
            aria-label="Sort field"
            className="mt-1 block h-9 rounded border bg-background px-3"
            value={view.sort?.field ?? ''}
            onChange={(event) =>
              update({
                sort: event.target.value
                  ? {
                      field: event.target.value,
                      descending: view.sort?.descending ?? false,
                    }
                  : null,
              })
            }
          >
            <option value="">Default order</option>
            {fields.map((field) => (
              <option key={field} value={field}>
                {stateLabel(field, schema[field])}
              </option>
            ))}
          </select>
        </label>
        {view.sort && (
          <label className="flex h-9 items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={view.sort.descending ?? false}
              onChange={(event) =>
                update({
                  sort: { ...view.sort!, descending: event.target.checked },
                })
              }
            />
            Descending
          </label>
        )}
      </div>
      <div className="grid gap-4 md:grid-cols-3">
        {(['key', 'stage', 'due'] as const).map((role) => (
          <label key={role} className="text-sm">
            {stateLabel(role)} field
            <select
              className="mt-1 block w-full rounded border bg-background p-2"
              value={view.roles?.[role] ?? ''}
              onChange={(e) =>
                update({
                  roles: { ...view.roles, [role]: e.target.value || null },
                })
              }
            >
              <option value="">{role === 'key' ? 'Run label' : 'None'}</option>
              {fields.map((field) => (
                <option key={field} value={field}>
                  {stateLabel(field, schema[field])}
                </option>
              ))}
            </select>
          </label>
        ))}
      </div>
      <fieldset className="space-y-2">
        <legend className="mb-2 text-sm font-medium">
          Columns and display
        </legend>
        {fields.map((field) => {
          const format = view.formats?.[field] ?? {};
          return (
            <div
              key={field}
              className="flex flex-wrap items-center gap-3 text-sm"
            >
              <label className="min-w-40">
                <input
                  type="checkbox"
                  checked={view.columns?.includes(field) ?? false}
                  onChange={(e) =>
                    update({
                      columns: e.target.checked
                        ? [...(view.columns ?? []), field]
                        : (view.columns ?? []).filter((f) => f !== field),
                    })
                  }
                />{' '}
                {stateLabel(field, schema[field])}
              </label>
              <Input
                className="w-36"
                aria-label={`${field} label`}
                placeholder="Label"
                value={view.labels?.[field] ?? ''}
                onChange={(e) =>
                  update({
                    labels: { ...view.labels, [field]: e.target.value },
                  })
                }
              />
              <select
                aria-label={`${field} display`}
                className="rounded border bg-background p-2"
                value={format.kind ?? ''}
                onChange={(e) =>
                  update({
                    formats: {
                      ...view.formats,
                      [field]: {
                        ...format,
                        kind: (e.target.value || null) as typeof format.kind,
                      },
                    },
                  })
                }
              >
                {['', 'text', 'number', 'date', 'datetime', 'relative'].map(
                  (kind) => (
                    <option key={kind} value={kind}>
                      {kind || 'Automatic'}
                    </option>
                  )
                )}
              </select>
              {format.kind === 'number' ? (
                <>
                  <Input
                    aria-label={`${field} decimals`}
                    className="w-24"
                    type="number"
                    min={0}
                    max={20}
                    placeholder="Decimals"
                    value={format.decimals ?? ''}
                    onChange={(e) =>
                      update({
                        formats: {
                          ...view.formats,
                          [field]: {
                            ...format,
                            decimals:
                              e.target.value === ''
                                ? null
                                : Number(e.target.value),
                          },
                        },
                      })
                    }
                  />
                  <Input
                    aria-label={`${field} prefix`}
                    className="w-24"
                    placeholder="Prefix"
                    value={format.prefix ?? ''}
                    onChange={(e) =>
                      update({
                        formats: {
                          ...view.formats,
                          [field]: { ...format, prefix: e.target.value },
                        },
                      })
                    }
                  />
                  <Input
                    aria-label={`${field} suffix`}
                    className="w-24"
                    placeholder="Suffix"
                    value={format.suffix ?? ''}
                    onChange={(e) =>
                      update({
                        formats: {
                          ...view.formats,
                          [field]: { ...format, suffix: e.target.value },
                        },
                      })
                    }
                  />
                </>
              ) : null}
            </div>
          );
        })}
      </fieldset>
      <StateFilters
        filters={view.where?.state ?? []}
        fields={fields}
        schema={schema}
        onChange={(state) => update({ where: { ...view.where, state } })}
      />
      {view.where?.openRequest ? (
        <div className="flex flex-wrap gap-4">
          <label className="text-sm">
            Inline answer field
            <Input
              placeholder="Leave empty to review each request"
              value={view.answers?.inline ?? ''}
              onChange={(e) =>
                update({
                  answers: { ...view.answers, inline: e.target.value || null },
                })
              }
            />
          </label>
          <label className="self-end text-sm">
            <input
              type="checkbox"
              checked={view.answers?.bulk ?? false}
              onChange={(e) =>
                update({ answers: { ...view.answers, bulk: e.target.checked } })
              }
            />{' '}
            Allow bulk answers
          </label>
        </div>
      ) : null}
      {error ? (
        <p role="alert" className="text-destructive">
          {error}
        </p>
      ) : null}
      <div className="flex gap-2">
        <Button
          disabled={saving || !view.name.trim()}
          onClick={() => void save()}
        >
          {saving ? 'Saving…' : saved ? 'Save changes' : 'Create queue'}
        </Button>
        <Button variant="secondary" disabled={saving} onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </section>
  );
}

export function StateFilters({
  filters,
  fields,
  schema,
  onChange,
}: {
  filters: StateFilterDto[];
  fields: string[];
  schema: Record<string, StateField>;
  onChange: (f: StateFilterDto[]) => void;
}) {
  const change = (index: number, patch: Partial<StateFilterDto>) =>
    onChange(filters.map((f, i) => (i === index ? { ...f, ...patch } : f)));
  return (
    <fieldset className="space-y-2">
      <legend className="text-sm font-medium">State filters</legend>
      {filters.map((filter, index) => {
        const relative = Boolean(
          filter.value &&
          typeof filter.value === 'object' &&
          'relative' in filter.value
        );
        return (
          <div key={index} className="flex flex-wrap items-center gap-2">
            <select
              aria-label={`Filter ${index + 1} field`}
              className="rounded border bg-background p-2"
              value={filter.field}
              onChange={(e) => change(index, { field: e.target.value })}
            >
              {fields.map((field) => (
                <option key={field} value={field}>
                  {stateLabel(field, schema[field])}
                </option>
              ))}
            </select>
            <select
              aria-label={`Filter ${index + 1} operator`}
              className="rounded border bg-background p-2"
              value={filter.op}
              onChange={(e) => change(index, { op: e.target.value })}
            >
              {['eq', 'ne', 'lt', 'lte', 'gt', 'gte', 'exists'].map((op) => (
                <option key={op}>{op}</option>
              ))}
            </select>
            <Input
              className="w-52"
              aria-label={`Filter ${index + 1} value`}
              disabled={relative}
              value={
                relative
                  ? 'Now'
                  : typeof filter.value === 'object'
                    ? JSON.stringify(filter.value)
                    : String(filter.value ?? '')
              }
              onChange={(e) => {
                let value: unknown = e.target.value;
                if (
                  schema[filter.field]?.type === 'number' ||
                  schema[filter.field]?.type === 'integer'
                )
                  value = e.target.value === '' ? '' : Number(e.target.value);
                if (
                  schema[filter.field]?.type === 'boolean' ||
                  filter.op === 'exists'
                )
                  value = e.target.value === 'true';
                change(index, { value: value as StateFilterDto['value'] });
              }}
            />
            <label className="text-sm">
              <input
                type="checkbox"
                checked={relative}
                onChange={(e) =>
                  change(index, {
                    value: (e.target.checked
                      ? { relative: 'now', offsetSeconds: 0 }
                      : '') as StateFilterDto['value'],
                  })
                }
              />{' '}
              Compare with now
            </label>
            <Button
              variant="secondary"
              size="sm"
              onClick={() => onChange(filters.filter((_, i) => i !== index))}
            >
              Remove
            </Button>
          </div>
        );
      })}
      <Button
        variant="secondary"
        size="sm"
        disabled={!fields.length || filters.length >= 16}
        onClick={() =>
          onChange([
            ...filters,
            {
              field: fields[0],
              op: 'eq',
              value: '' as unknown as StateFilterDto['value'],
            },
          ])
        }
      >
        Add filter
      </Button>
    </fieldset>
  );
}
