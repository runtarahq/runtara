import { ReactNode, useEffect, useMemo, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router';
import { Plus, Save, Trash2 } from 'lucide-react';
import { Schema } from '@/generated/RuntaraRuntimeApi';
import { Badge } from '@/shared/components/ui/badge';
import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import { Textarea } from '@/shared/components/ui/textarea';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { TileList, TilesPage } from '@/shared/components/tiles-page';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { useObjectSchemaDtos } from '@/features/objects/hooks/useObjectSchemas';
import {
  useCreateReport,
  useReport,
  useUpdateReport,
} from '../hooks/useReports';
import { ReportDefinitionBuilder } from '../components/ReportDefinitionBuilder';
import {
  ReportAggregateFn,
  ReportDatasetDefinition,
  ReportDatasetFieldType,
  ReportDatasetValueFormat,
  ReportDefinition,
  ReportStatus,
} from '../types';
import {
  extractBlockPlaceholders,
  extractLayoutBlockReferences,
  humanizeFieldName,
  slugify,
} from '../utils';

const EMPTY_DEFINITION: ReportDefinition = {
  definitionVersion: 1,
  markdown: '# Report',
  layout: [{ id: 'intro', type: 'markdown', content: '# Report' }],
  filters: [],
  blocks: [],
};

const NONE_SELECT_VALUE = '__none__';

const DATASET_FIELD_TYPES: ReportDatasetFieldType[] = [
  'string',
  'number',
  'decimal',
  'boolean',
  'date',
  'datetime',
  'json',
];

const DATASET_FORMATS: ReportDatasetValueFormat[] = [
  'string',
  'number',
  'decimal',
  'currency',
  'percent',
  'boolean',
  'date',
  'datetime',
];

const DATASET_MEASURE_OPS: ReportAggregateFn[] = [
  'count',
  'sum',
  'avg',
  'min',
  'max',
];

export function ReportEditorPage() {
  const { reportId } = useParams();
  const isEditing = Boolean(reportId);
  const navigate = useNavigate();
  const { data: existingReport, isFetching } = useReport(reportId);
  const { data: schemas = [] } = useObjectSchemaDtos();
  const createReport = useCreateReport();
  const updateReport = useUpdateReport();

  usePageTitle(isEditing ? 'Edit Report' : 'New Report');

  const [name, setName] = useState('');
  const [slug, setSlug] = useState('');
  const [description, setDescription] = useState('');
  const [status, setStatus] = useState<ReportStatus>('published');
  const [definition, setDefinition] =
    useState<ReportDefinition>(EMPTY_DEFINITION);
  const [selectedSchema, setSelectedSchema] = useState('');
  const [localError, setLocalError] = useState<string | null>(null);

  useEffect(() => {
    if (!existingReport) return;
    setName(existingReport.name);
    setSlug(existingReport.slug);
    setDescription(existingReport.description ?? '');
    setStatus(existingReport.status);
    setDefinition(existingReport.definition);
  }, [existingReport]);

  useEffect(() => {
    if (selectedSchema || schemas.length === 0) return;
    setSelectedSchema(schemas[0]?.name ?? '');
  }, [schemas, selectedSchema]);

  const definitionErrors = useMemo(
    () => validateReportDefinition(definition),
    [definition]
  );

  const canSave =
    name.trim().length > 0 &&
    slug.trim().length > 0 &&
    definitionErrors.length === 0 &&
    !createReport.isPending &&
    !updateReport.isPending;

  const handleNameChange = (value: string) => {
    setName(value);
    if (!isEditing || slug.length === 0) {
      setSlug(slugify(value));
    }
  };

  const handleGenerateStarter = () => {
    const schema = schemas.find(
      (candidate) => candidate.name === selectedSchema
    );
    if (!schema) return;

    const columns = schema.columns
      .filter((column) => column.name !== 'id')
      .slice(0, 6)
      .map((column) => ({
        field: column.name,
        label: column.name
          .split(/[_-]/)
          .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
          .join(' '),
      }));

    const starter: ReportDefinition = {
      definitionVersion: 1,
      markdown: `# ${name || schema.name}\n\n{{ block.total_records }}\n\n{{ block.records }}`,
      layout: [
        {
          id: 'intro',
          type: 'markdown',
          content: `# ${name || schema.name}`,
        },
        {
          id: 'summary_metrics',
          type: 'metric_row',
          blocks: ['total_records'],
        },
        {
          id: 'records_node',
          type: 'block',
          blockId: 'records',
        },
      ],
      filters: [],
      blocks: [
        {
          id: 'total_records',
          type: 'metric',
          title: 'Total records',
          source: {
            schema: schema.name,
            mode: 'aggregate',
            aggregates: [{ alias: 'value', op: 'count' }],
          },
          metric: {
            valueField: 'value',
            label: 'Total records',
            format: 'number',
          },
        },
        {
          id: 'records',
          type: 'table',
          title: 'Records',
          lazy: false,
          source: {
            schema: schema.name,
            mode: 'filter',
          },
          table: {
            columns,
            pagination: {
              defaultPageSize: 50,
              allowedPageSizes: [25, 50, 100],
            },
          },
        },
      ],
    };

    setDefinition(starter);
    setLocalError(null);
  };

  const handleSave = async () => {
    if (definitionErrors.length > 0) {
      setLocalError(definitionErrors[0]);
      return;
    }

    setLocalError(null);
    const payload = {
      name: name.trim(),
      slug: slug.trim(),
      description: description.trim() || null,
      tags: [],
      status,
      definition,
    };

    if (isEditing && reportId) {
      const report = await updateReport.mutateAsync({
        id: reportId,
        data: payload,
      });
      navigate(`/reports/${report.id}`);
    } else {
      const report = await createReport.mutateAsync(payload);
      navigate(`/reports/${report.id}`);
    }
  };

  if (isEditing && isFetching) {
    return (
      <TilesPage kicker="Reports" title="Loading report">
        <TileList>
          <div className="h-96 animate-pulse rounded-xl bg-muted/30" />
        </TileList>
      </TilesPage>
    );
  }

  return (
    <TilesPage
      kicker="Reports"
      title={isEditing ? 'Edit report' : 'New report'}
      action={
        <div className="flex w-full flex-col gap-2 sm:w-auto sm:flex-row">
          <Link
            to={isEditing && reportId ? `/reports/${reportId}` : '/reports'}
          >
            <Button
              variant="outline"
              className="h-11 w-full rounded-full sm:px-5"
            >
              Cancel
            </Button>
          </Link>
          <Button
            className="h-11 rounded-full sm:px-5"
            disabled={!canSave}
            onClick={handleSave}
          >
            <Save className="mr-2 h-4 w-4" />
            Save
          </Button>
        </div>
      }
    >
      <div className="grid gap-5 xl:grid-cols-[minmax(280px,360px)_1fr]">
        <section className="space-y-4 rounded-lg border bg-background p-4">
          <div className="space-y-2">
            <Label htmlFor="report-name">Name</Label>
            <Input
              id="report-name"
              value={name}
              onChange={(event) => handleNameChange(event.target.value)}
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="report-slug">Slug</Label>
            <Input
              id="report-slug"
              value={slug}
              onChange={(event) => setSlug(slugify(event.target.value))}
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="report-description">Description</Label>
            <Textarea
              id="report-description"
              value={description}
              onChange={(event) => setDescription(event.target.value)}
            />
          </div>
          <div className="space-y-2">
            <Label>Status</Label>
            <Select
              value={status}
              onValueChange={(value) => setStatus(value as ReportStatus)}
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="draft">Draft</SelectItem>
                <SelectItem value="published">Published</SelectItem>
                <SelectItem value="archived">Archived</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="space-y-2 rounded-lg bg-muted/30 p-3">
            <Label>Starter schema</Label>
            <Select value={selectedSchema} onValueChange={setSelectedSchema}>
              <SelectTrigger>
                <SelectValue placeholder="Select schema" />
              </SelectTrigger>
              <SelectContent>
                {schemas.map((schema) => (
                  <SelectItem key={schema.id} value={schema.name}>
                    {schema.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Button
              type="button"
              variant="outline"
              className="w-full"
              disabled={!selectedSchema}
              onClick={handleGenerateStarter}
            >
              Generate starter definition
            </Button>
          </div>
        </section>

        <div className="flex flex-col gap-3">
          <ReportDatasetsEditor
            definition={definition}
            schemas={schemas}
            onChange={(nextDefinition) => {
              setDefinition(nextDefinition);
              setLocalError(null);
            }}
          />
          <ReportDefinitionBuilder
            value={definition}
            schemas={schemas}
            selectedSchema={selectedSchema}
            onSelectedSchemaChange={setSelectedSchema}
            onChange={(nextDefinition) => {
              setDefinition(nextDefinition);
              setLocalError(null);
            }}
          />
          {(localError || definitionErrors.length > 0) && (
            <p className="text-sm text-destructive">
              {localError ?? definitionErrors[0]}
            </p>
          )}
        </div>
      </div>
    </TilesPage>
  );
}

function ReportDatasetsEditor({
  definition,
  schemas,
  onChange,
}: {
  definition: ReportDefinition;
  schemas: Schema[];
  onChange: (definition: ReportDefinition) => void;
}) {
  const datasets = definition.datasets ?? [];

  const updateDatasets = (
    nextDatasets: ReportDatasetDefinition[],
    rename?: { previousId: string; nextId: string }
  ) => {
    const blocks =
      rename && rename.previousId !== rename.nextId
        ? definition.blocks.map((block) =>
            block.dataset?.id === rename.previousId
              ? {
                  ...block,
                  dataset: { ...block.dataset, id: rename.nextId },
                }
              : block
          )
        : definition.blocks;
    onChange({ ...definition, datasets: nextDatasets, blocks });
  };

  const updateDataset = (
    index: number,
    nextDataset: ReportDatasetDefinition
  ) => {
    const previousId = datasets[index]?.id ?? '';
    const nextDatasets = datasets.map((dataset, currentIndex) =>
      currentIndex === index ? nextDataset : dataset
    );
    updateDatasets(nextDatasets, {
      previousId,
      nextId: nextDataset.id,
    });
  };

  const addDataset = () => {
    const schema = schemas[0];
    const dataset = createDefaultDataset(schema, datasets);
    updateDatasets([...datasets, dataset]);
  };

  const removeDataset = (index: number) => {
    updateDatasets(datasets.filter((_, currentIndex) => currentIndex !== index));
  };

  return (
    <section className="flex flex-col gap-4 rounded-lg border bg-background p-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-center gap-2">
          <h2 className="text-base font-semibold text-foreground">
            Semantic datasets
          </h2>
          <Badge variant="secondary">{datasets.length} datasets</Badge>
        </div>
        <Button type="button" variant="outline" size="sm" onClick={addDataset}>
          <Plus className="mr-2 size-4" />
          Add dataset
        </Button>
      </div>

      {datasets.length === 0 ? (
        <div className="rounded-lg border border-dashed bg-muted/20 p-6 text-sm text-muted-foreground">
          No semantic datasets.
        </div>
      ) : (
        <div className="flex flex-col gap-3">
          {datasets.map((dataset, index) => (
            <DatasetEditorCard
              key={`dataset-${index}`}
              dataset={dataset}
              schemas={schemas}
              onChange={(nextDataset) => updateDataset(index, nextDataset)}
              onRemove={() => removeDataset(index)}
            />
          ))}
        </div>
      )}
    </section>
  );
}

function DatasetEditorCard({
  dataset,
  schemas,
  onChange,
  onRemove,
}: {
  dataset: ReportDatasetDefinition;
  schemas: Schema[];
  onChange: (dataset: ReportDatasetDefinition) => void;
  onRemove: () => void;
}) {
  const schema = schemas.find(
    (candidate) => candidate.name === dataset.source.schema
  );
  const schemaFields = getSchemaFieldNames(schema);
  const dimensionFields = new Set(
    dataset.dimensions.map((dimension) => dimension.field)
  );
  const measureIds = new Set(dataset.measures.map((measure) => measure.id));

  const updateDimension = (
    field: string,
    patch: Partial<ReportDatasetDefinition['dimensions'][number]>
  ) => {
    onChange({
      ...dataset,
      dimensions: dataset.dimensions.map((dimension) =>
        dimension.field === field ? { ...dimension, ...patch } : dimension
      ),
    });
  };

  const updateMeasure = (
    id: string,
    patch: Partial<ReportDatasetDefinition['measures'][number]>
  ) => {
    onChange({
      ...dataset,
      measures: dataset.measures.map((measure) =>
        measure.id === id ? { ...measure, ...patch } : measure
      ),
    });
  };

  const addDimension = (field: string) => {
    if (field === NONE_SELECT_VALUE || dimensionFields.has(field)) return;
    onChange({
      ...dataset,
      dimensions: [
        ...dataset.dimensions,
        {
          field,
          label: humanizeFieldName(field),
          type: inferDatasetFieldType(field),
          format: inferDatasetFormat(field),
        },
      ],
    });
  };

  const addCountMeasure = () => {
    const id = uniqueDatasetFieldId(measureIds, 'record_count');
    onChange({
      ...dataset,
      measures: [
        ...dataset.measures,
        {
          id,
          label: 'Record count',
          op: 'count',
          format: 'number',
        },
      ],
    });
  };

  const addFieldMeasure = (field: string) => {
    if (field === NONE_SELECT_VALUE) return;
    const id = uniqueDatasetFieldId(measureIds, `${field}_total`);
    onChange({
      ...dataset,
      measures: [
        ...dataset.measures,
        {
          id,
          label: `Total ${humanizeFieldName(field).toLowerCase()}`,
          op: 'sum',
          field,
          format: 'number',
        },
      ],
    });
  };

  const removeDimension = (field: string) => {
    onChange({
      ...dataset,
      timeDimension:
        dataset.timeDimension === field ? undefined : dataset.timeDimension,
      dimensions: dataset.dimensions.filter(
        (dimension) => dimension.field !== field
      ),
    });
  };

  const removeMeasure = (id: string) => {
    onChange({
      ...dataset,
      measures: dataset.measures.filter((measure) => measure.id !== id),
    });
  };

  return (
    <div className="rounded-lg border bg-muted/10 p-4">
      <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
        <div className="min-w-0">
          <div className="truncate text-sm font-semibold text-foreground">
            {dataset.label || dataset.id}
          </div>
          <div className="text-xs text-muted-foreground">{dataset.id}</div>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="size-9"
          onClick={onRemove}
          aria-label="Remove dataset"
        >
          <Trash2 className="size-4" />
        </Button>
      </div>

      <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-4">
        <EditorField label="Dataset ID">
          <Input
            value={dataset.id}
            onChange={(event) =>
              onChange({
                ...dataset,
                id: slugify(event.target.value).replace(/-/g, '_'),
              })
            }
          />
        </EditorField>
        <EditorField label="Label">
          <Input
            value={dataset.label}
            onChange={(event) =>
              onChange({ ...dataset, label: event.target.value })
            }
          />
        </EditorField>
        <EditorField label="Source schema">
          <Select
            value={dataset.source.schema}
            onValueChange={(schemaName) =>
              onChange({
                ...dataset,
                source: { ...dataset.source, schema: schemaName },
              })
            }
          >
            <SelectTrigger>
              <SelectValue placeholder="Select schema" />
            </SelectTrigger>
            <SelectContent>
              {schemas.map((schemaOption) => (
                <SelectItem key={schemaOption.id} value={schemaOption.name}>
                  {schemaOption.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </EditorField>
        <EditorField label="Time dimension">
          <Select
            value={dataset.timeDimension ?? NONE_SELECT_VALUE}
            onValueChange={(field) =>
              onChange({
                ...dataset,
                timeDimension:
                  field === NONE_SELECT_VALUE ? undefined : field,
              })
            }
          >
            <SelectTrigger>
              <SelectValue placeholder="No time dimension" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={NONE_SELECT_VALUE}>None</SelectItem>
              {dataset.dimensions.map((dimension) => (
                <SelectItem key={dimension.field} value={dimension.field}>
                  {dimension.label || dimension.field}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </EditorField>
      </div>

      <div className="mt-4 grid gap-4 xl:grid-cols-2">
        <div className="space-y-3">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <h3 className="text-sm font-semibold text-foreground">
              Dimensions
            </h3>
            <Select value={NONE_SELECT_VALUE} onValueChange={addDimension}>
              <SelectTrigger className="h-9 w-48">
                <SelectValue placeholder="Add dimension" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={NONE_SELECT_VALUE} disabled>
                  Add dimension
                </SelectItem>
                {schemaFields.map((field) => (
                  <SelectItem
                    key={field}
                    value={field}
                    disabled={dimensionFields.has(field)}
                  >
                    {field}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          {dataset.dimensions.length === 0 ? (
            <p className="rounded-md border border-dashed bg-background p-3 text-sm text-muted-foreground">
              No dimensions.
            </p>
          ) : (
            <div className="space-y-2">
              {dataset.dimensions.map((dimension) => (
                <div
                  key={dimension.field}
                  className="grid gap-2 rounded-md border bg-background p-3 md:grid-cols-[1fr_1fr_8rem_8rem_auto]"
                >
                  <Input value={dimension.field} readOnly className="bg-muted/40" />
                  <Input
                    value={dimension.label}
                    onChange={(event) =>
                      updateDimension(dimension.field, {
                        label: event.target.value,
                      })
                    }
                  />
                  <Select
                    value={dimension.type}
                    onValueChange={(type) =>
                      updateDimension(dimension.field, {
                        type: type as ReportDatasetFieldType,
                      })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {DATASET_FIELD_TYPES.map((type) => (
                        <SelectItem key={type} value={type}>
                          {type}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <Select
                    value={dimension.format ?? NONE_SELECT_VALUE}
                    onValueChange={(format) =>
                      updateDimension(dimension.field, {
                        format:
                          format === NONE_SELECT_VALUE
                            ? undefined
                            : (format as ReportDatasetValueFormat),
                      })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value={NONE_SELECT_VALUE}>No format</SelectItem>
                      {DATASET_FORMATS.map((format) => (
                        <SelectItem key={format} value={format}>
                          {format}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    onClick={() => removeDimension(dimension.field)}
                    aria-label={`Remove ${dimension.field}`}
                  >
                    <Trash2 className="size-4" />
                  </Button>
                </div>
              ))}
            </div>
          )}
        </div>

        <div className="space-y-3">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <h3 className="text-sm font-semibold text-foreground">Measures</h3>
            <div className="flex flex-wrap gap-2">
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={addCountMeasure}
              >
                <Plus className="mr-2 size-4" />
                Count
              </Button>
              <Select value={NONE_SELECT_VALUE} onValueChange={addFieldMeasure}>
                <SelectTrigger className="h-9 w-44">
                  <SelectValue placeholder="Add measure" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value={NONE_SELECT_VALUE} disabled>
                    Add measure
                  </SelectItem>
                  {schemaFields.map((field) => (
                    <SelectItem key={field} value={field}>
                      {field}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </div>
          {dataset.measures.length === 0 ? (
            <p className="rounded-md border border-dashed bg-background p-3 text-sm text-muted-foreground">
              No measures.
            </p>
          ) : (
            <div className="space-y-2">
              {dataset.measures.map((measure, measureIndex) => (
                <div
                  key={`measure-${measureIndex}`}
                  className="grid gap-2 rounded-md border bg-background p-3 md:grid-cols-[1fr_1fr_7rem_1fr_8rem_auto]"
                >
                  <Input
                    value={measure.id}
                    onChange={(event) =>
                      updateMeasure(measure.id, {
                        id: slugify(event.target.value).replace(/-/g, '_'),
                      })
                    }
                  />
                  <Input
                    value={measure.label}
                    onChange={(event) =>
                      updateMeasure(measure.id, { label: event.target.value })
                    }
                  />
                  <Select
                    value={measure.op}
                    onValueChange={(op) =>
                      updateMeasure(measure.id, {
                        op: op as ReportAggregateFn,
                      })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {DATASET_MEASURE_OPS.map((op) => (
                        <SelectItem key={op} value={op}>
                          {op}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <Select
                    value={measure.field ?? NONE_SELECT_VALUE}
                    onValueChange={(field) =>
                      updateMeasure(measure.id, {
                        field:
                          field === NONE_SELECT_VALUE ? undefined : field,
                      })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value={NONE_SELECT_VALUE}>No field</SelectItem>
                      {schemaFields.map((field) => (
                        <SelectItem key={field} value={field}>
                          {field}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <Select
                    value={measure.format}
                    onValueChange={(format) =>
                      updateMeasure(measure.id, {
                        format: format as ReportDatasetValueFormat,
                      })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {DATASET_FORMATS.map((format) => (
                        <SelectItem key={format} value={format}>
                          {format}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    onClick={() => removeMeasure(measure.id)}
                    aria-label={`Remove ${measure.id}`}
                  >
                    <Trash2 className="size-4" />
                  </Button>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function EditorField({
  label,
  children,
}: {
  label: string;
  children: ReactNode;
}) {
  return (
    <div className="space-y-1">
      <Label className="text-xs text-muted-foreground">{label}</Label>
      {children}
    </div>
  );
}

function createDefaultDataset(
  schema: Schema | undefined,
  existingDatasets: ReportDatasetDefinition[]
): ReportDatasetDefinition {
  const schemaName = schema?.name ?? '';
  const fields = getSchemaFieldNames(schema);
  const preferredTimeField =
    fields.find((field) => /date|time|created|updated/i.test(field)) ??
    undefined;
  const dimensions = fields.slice(0, 4).map((field) => ({
    field,
    label: humanizeFieldName(field),
    type: inferDatasetFieldType(field),
    format: inferDatasetFormat(field),
  }));

  return {
    id: uniqueDatasetId(
      existingDatasets,
      slugify(schemaName || 'dataset').replace(/-/g, '_')
    ),
    label: schemaName ? humanizeFieldName(schemaName) : 'Dataset',
    source: { schema: schemaName, connectionId: null },
    timeDimension: preferredTimeField,
    dimensions,
    measures: [
      {
        id: 'record_count',
        label: 'Record count',
        op: 'count',
        format: 'number',
      },
    ],
  };
}

function getSchemaFieldNames(schema: Schema | undefined): string[] {
  return (schema?.columns ?? [])
    .map((column) => column.name)
    .filter((field) => field && field !== 'id');
}

function inferDatasetFieldType(field: string): ReportDatasetFieldType {
  if (/date|_at$|time/i.test(field)) return 'date';
  if (/is_|has_|enabled|active/i.test(field)) return 'boolean';
  if (/amount|price|cost|total|qty|quantity|count|number|value/i.test(field)) {
    return 'number';
  }
  return 'string';
}

function inferDatasetFormat(field: string): ReportDatasetValueFormat {
  if (/date|_at$|time/i.test(field)) return 'date';
  if (/amount|price|cost/i.test(field)) return 'currency';
  if (/qty|quantity|count|number|total|value/i.test(field)) return 'number';
  return 'string';
}

function uniqueDatasetId(
  datasets: ReportDatasetDefinition[],
  baseId: string
): string {
  const fallback = baseId || 'dataset';
  const existing = new Set(datasets.map((dataset) => dataset.id));
  if (!existing.has(fallback)) return fallback;
  let index = 2;
  while (existing.has(`${fallback}_${index}`)) {
    index += 1;
  }
  return `${fallback}_${index}`;
}

function uniqueDatasetFieldId(existing: Set<string>, baseId: string): string {
  const fallback = baseId || 'field';
  if (!existing.has(fallback)) return fallback;
  let index = 2;
  while (existing.has(`${fallback}_${index}`)) {
    index += 1;
  }
  return `${fallback}_${index}`;
}

function validateReportDefinition(definition: ReportDefinition): string[] {
  const errors: string[] = [];
  const blockIds = new Set<string>();
  const datasetIds = new Set<string>();

  for (const dataset of definition.datasets ?? []) {
    if (!dataset.id.trim()) {
      errors.push('Every semantic dataset needs an ID.');
      continue;
    }
    if (datasetIds.has(dataset.id)) {
      errors.push(`Duplicate semantic dataset ID: ${dataset.id}`);
    }
    datasetIds.add(dataset.id);

    if (!dataset.label.trim()) {
      errors.push(`Dataset "${dataset.id}" needs a label.`);
    }
    if (!dataset.source.schema.trim()) {
      errors.push(`Dataset "${dataset.id}" needs a source schema.`);
    }

    const dimensionFields = new Set<string>();
    for (const dimension of dataset.dimensions) {
      if (!dimension.field.trim()) {
        errors.push(`Dataset "${dataset.id}" has a dimension without a field.`);
      }
      if (dimensionFields.has(dimension.field)) {
        errors.push(
          `Dataset "${dataset.id}" has duplicate dimension: ${dimension.field}`
        );
      }
      dimensionFields.add(dimension.field);
      if (!dimension.label.trim()) {
        errors.push(
          `Dataset "${dataset.id}" dimension "${dimension.field}" needs a label.`
        );
      }
    }

    if (dataset.timeDimension && !dimensionFields.has(dataset.timeDimension)) {
      errors.push(
        `Dataset "${dataset.id}" time dimension is not in dimensions.`
      );
    }

    const measureIds = new Set<string>();
    for (const measure of dataset.measures) {
      if (!measure.id.trim()) {
        errors.push(`Dataset "${dataset.id}" has a measure without an ID.`);
      }
      if (measureIds.has(measure.id)) {
        errors.push(
          `Dataset "${dataset.id}" has duplicate measure: ${measure.id}`
        );
      }
      measureIds.add(measure.id);
      if (!measure.label.trim()) {
        errors.push(
          `Dataset "${dataset.id}" measure "${measure.id}" needs a label.`
        );
      }
      if (measure.op !== 'count' && !measure.field?.trim()) {
        errors.push(
          `Dataset "${dataset.id}" measure "${measure.id}" needs a field.`
        );
      }
    }
  }

  for (const block of definition.blocks) {
    if (!block.id.trim()) {
      errors.push('Every report block needs an ID.');
      continue;
    }
    if (blockIds.has(block.id)) {
      errors.push(`Duplicate report block ID: ${block.id}`);
    }
    blockIds.add(block.id);
    if (!block.dataset && !block.source.schema.trim()) {
      errors.push(`Block "${block.id}" needs a schema.`);
    }
    if (block.dataset && !datasetIds.has(block.dataset.id)) {
      errors.push(
        `Block "${block.id}" references unknown dataset: ${block.dataset.id}`
      );
    }
  }

  for (const placeholder of extractBlockPlaceholders(definition.markdown)) {
    if (!blockIds.has(placeholder)) {
      errors.push(`Markdown references unknown block: ${placeholder}`);
    }
  }

  for (const blockId of extractLayoutBlockReferences(definition.layout)) {
    if (!blockIds.has(blockId)) {
      errors.push(`Layout references unknown block: ${blockId}`);
    }
  }

  return errors;
}
