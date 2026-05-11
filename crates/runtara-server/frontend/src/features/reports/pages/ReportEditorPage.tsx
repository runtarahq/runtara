import { ReactNode, useEffect, useMemo, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router';
import {
  AlertTriangle,
  CheckCircle2,
  Filter,
  Plus,
  RefreshCw,
  Save,
  Trash2,
} from 'lucide-react';
import { Schema } from '@/generated/RuntaraRuntimeApi';
import {
  Alert,
  AlertDescription,
  AlertTitle,
} from '@/shared/components/ui/alert';
import { Badge } from '@/shared/components/ui/badge';
import { Button } from '@/shared/components/ui/button';
import { Checkbox } from '@/shared/components/ui/checkbox';
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
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from '@/shared/components/ui/tabs';
import { TileList, TilesPage } from '@/shared/components/tiles-page';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { useObjectSchemaDtos } from '@/features/objects/hooks/useObjectSchemas';
import {
  useCreateReport,
  useReport,
  useReportPreview,
  useUpdateReport,
  useValidateReport,
} from '../hooks/useReports';
import { ReportDefinitionBuilder } from '../components/ReportDefinitionBuilder';
import { ReportDeleteButton } from '../components/ReportDeleteButton';
import { ReportFilterBar } from '../components/ReportFilterBar';
import { ReportRenderer } from '../components/ReportRenderer';
import {
  ReportAggregateFn,
  ReportBlockDefinition,
  ReportCardConfig,
  ReportDatasetDefinition,
  ReportDatasetFieldType,
  ReportDatasetValueFormat,
  ReportDefinition,
  ReportFilterDefinition,
  ReportFilterType,
  ReportStatus,
  ReportValidationIssue,
} from '../types';
import {
  extractLayoutBlockReferences,
  getActiveReportLayout,
  getFilterDefaultValue,
  humanizeFieldName,
  isVisibleByShowWhen,
  slugify,
} from '../utils';
import { reconcileDatasetBlock } from '../datasetBlocks';

const EMPTY_DEFINITION: ReportDefinition = {
  definitionVersion: 1,
  layout: [{ id: 'intro_node', type: 'block', blockId: 'intro' }],
  filters: [],
  blocks: [
    {
      id: 'intro',
      type: 'markdown',
      source: { schema: '', mode: 'filter' },
      markdown: { content: '# Report' },
      filters: [],
    },
  ],
};

const NONE_SELECT_VALUE = '__none__';
const ALL_TARGETS_SELECT_VALUE = '__all_targets__';

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

const FILTER_TYPE_OPTIONS: Array<{
  label: string;
  value: ReportFilterType;
}> = [
  { label: 'Select', value: 'select' },
  { label: 'Multi-select', value: 'multi_select' },
  { label: 'Radio', value: 'radio' },
  { label: 'Checkbox', value: 'checkbox' },
  { label: 'Time range', value: 'time_range' },
  { label: 'Number range', value: 'number_range' },
  { label: 'Text', value: 'text' },
  { label: 'Search', value: 'search' },
];

const FILTER_OPERATOR_OPTIONS = [
  { label: 'Equals', value: 'eq' },
  { label: 'Not equals', value: 'ne' },
  { label: 'Contains', value: 'contains' },
  { label: 'Search', value: 'search' },
  { label: 'In', value: 'in' },
  { label: 'Greater than', value: 'gt' },
  { label: 'Greater or equal', value: 'gte' },
  { label: 'Less than', value: 'lt' },
  { label: 'Less or equal', value: 'lte' },
  { label: 'Between', value: 'between' },
];

export function ReportEditorPage() {
  const { reportId } = useParams();
  const isEditing = Boolean(reportId);
  const navigate = useNavigate();
  const { data: existingReport, isFetching } = useReport(reportId);
  const { data: schemas = [] } = useObjectSchemaDtos();
  const createReport = useCreateReport();
  const updateReport = useUpdateReport();
  const validateReport = useValidateReport();

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
    setSelectedSchema(inferReportPrimarySchema(existingReport.definition));
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
    !updateReport.isPending &&
    !validateReport.isPending;

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
      layout: [
        {
          id: 'intro_node',
          type: 'block',
          blockId: 'intro',
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
          id: 'intro',
          type: 'markdown',
          source: { schema: '', mode: 'filter' },
          markdown: { content: `# ${name || schema.name}` },
          filters: [],
        },
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
    validateReport.reset();
  };

  const handleValidate = async () => {
    setLocalError(null);
    await validateReport.mutateAsync({ definition });
  };

  const handleSave = async () => {
    if (definitionErrors.length > 0) {
      setLocalError(definitionErrors[0]);
      return;
    }

    setLocalError(null);
    const validation = await validateReport.mutateAsync({ definition });
    if (!validation.valid) {
      setLocalError(validation.errors[0]?.message ?? 'Report is invalid.');
      return;
    }

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
          {isEditing && reportId && existingReport ? (
            <ReportDeleteButton
              reportId={reportId}
              reportName={existingReport.name}
              className="h-11 rounded-full sm:px-5"
            />
          ) : null}
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
            <Label>Default schema for new blocks and datasets</Label>
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
          <Tabs defaultValue="build" className="w-full">
            <TabsList className="report-print-hidden">
              <TabsTrigger value="build">Build</TabsTrigger>
              <TabsTrigger value="preview">Preview</TabsTrigger>
              <TabsTrigger value="validation">Validation</TabsTrigger>
            </TabsList>
            <TabsContent value="build" className="mt-3 flex flex-col gap-3">
              <ReportFiltersEditor
                definition={definition}
                schemas={schemas}
                onChange={(nextDefinition) => {
                  setDefinition(nextDefinition);
                  setLocalError(null);
                  validateReport.reset();
                }}
              />
              <ReportDatasetsEditor
                definition={definition}
                schemas={schemas}
                selectedSchema={selectedSchema}
                onChange={(nextDefinition) => {
                  setDefinition(nextDefinition);
                  setLocalError(null);
                  validateReport.reset();
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
                  validateReport.reset();
                }}
              />
            </TabsContent>
            <TabsContent value="preview" className="mt-3">
              <ReportPreviewPanel definition={definition} />
            </TabsContent>
            <TabsContent value="validation" className="mt-3">
              <ReportValidationPanel
                localErrors={definitionErrors}
                serverErrors={validateReport.data?.errors ?? []}
                serverWarnings={validateReport.data?.warnings ?? []}
                isValid={validateReport.data?.valid}
                isPending={validateReport.isPending}
                onValidate={handleValidate}
              />
            </TabsContent>
          </Tabs>
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

function ReportFiltersEditor({
  definition,
  schemas,
  onChange,
}: {
  definition: ReportDefinition;
  schemas: Schema[];
  onChange: (definition: ReportDefinition) => void;
}) {
  const filters = definition.filters ?? [];
  const filterableBlocks = definition.blocks.filter((block) =>
    canMapFilterToBlock(definition, block.id, schemas)
  );

  const updateFilters = (nextFilters: ReportFilterDefinition[]) => {
    onChange({ ...definition, filters: nextFilters });
  };

  const updateFilter = (index: number, filter: ReportFilterDefinition) => {
    updateFilters(
      filters.map((current, currentIndex) =>
        currentIndex === index ? normalizeFilterForType(filter) : current
      )
    );
  };

  const addFilter = () => {
    updateFilters([...filters, createDefaultReportFilter(definition, schemas)]);
  };

  return (
    <section className="flex flex-col gap-4 rounded-lg border bg-background p-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-center gap-2">
          <h2 className="text-base font-semibold text-foreground">
            Report filters
          </h2>
          <Badge variant="secondary">{filters.length} filters</Badge>
        </div>
        <Button type="button" variant="outline" size="sm" onClick={addFilter}>
          <Filter className="mr-2 size-4" />
          Add filter
        </Button>
      </div>

      {filters.length === 0 ? (
        <div className="rounded-lg border border-dashed bg-muted/20 p-6 text-sm text-muted-foreground">
          No report-level filters. Add filters here when viewers should control
          multiple blocks from the report header.
        </div>
      ) : (
        <div className="flex flex-col gap-3">
          {filters.map((filter, index) => (
            <ReportFilterEditorCard
              key={`filter-${index}-${filter.id}`}
              filter={filter}
              definition={definition}
              schemas={schemas}
              filterableBlocks={filterableBlocks}
              onChange={(nextFilter) => updateFilter(index, nextFilter)}
              onRemove={() =>
                updateFilters(
                  filters.filter((_, currentIndex) => currentIndex !== index)
                )
              }
            />
          ))}
        </div>
      )}
    </section>
  );
}

function ReportFilterEditorCard({
  filter,
  definition,
  schemas,
  filterableBlocks,
  onChange,
  onRemove,
}: {
  filter: ReportFilterDefinition;
  definition: ReportDefinition;
  schemas: Schema[];
  filterableBlocks: ReportDefinition['blocks'];
  onChange: (filter: ReportFilterDefinition) => void;
  onRemove: () => void;
}) {
  const mappings = filter.appliesTo ?? [];
  const optionsSource = filter.options?.source ?? 'static';
  const optionsSchemaName =
    filter.options?.schema ??
    getBlockSourceSchema(definition, mappings[0]?.blockId ?? '') ??
    schemas[0]?.name ??
    '';
  const optionSchema = schemas.find(
    (schema) => schema.name === optionsSchemaName
  );
  const optionFields = getSchemaFieldNames(optionSchema);
  const valueField = filter.options?.valueField ?? filter.options?.field ?? '';
  const labelField = filter.options?.labelField ?? valueField;

  const updateMapping = (
    index: number,
    patch: Partial<NonNullable<ReportFilterDefinition['appliesTo']>[number]>
  ) => {
    onChange({
      ...filter,
      appliesTo: mappings.map((mapping, currentIndex) =>
        currentIndex === index ? { ...mapping, ...patch } : mapping
      ),
    });
  };

  const updateMappingTarget = (index: number, targetValue: string) => {
    const mapping = mappings[index];
    const blockId =
      targetValue === ALL_TARGETS_SELECT_VALUE ? undefined : targetValue;
    const fields = getFilterMappingFields(
      definition,
      blockId,
      filterableBlocks,
      schemas
    );
    const nextField = fields.includes(mapping?.field ?? '')
      ? (mapping?.field ?? '')
      : (fields[0] ?? mapping?.field ?? '');
    updateMapping(index, {
      blockId,
      field: nextField,
      op: mapping?.op ?? defaultFilterOperator(filter.type),
    });
  };

  const addMapping = () => {
    const block = filterableBlocks[0];
    const fields = block
      ? getFilterFieldNames(definition, block.id, schemas)
      : getGlobalFilterFieldNames(definition, filterableBlocks, schemas);
    const field = fields[0] ?? 'id';
    onChange({
      ...filter,
      appliesTo: [
        ...mappings,
        { blockId: block?.id, field, op: defaultFilterOperator(filter.type) },
      ],
    });
  };

  const removeMapping = (index: number) => {
    onChange({
      ...filter,
      appliesTo: mappings.filter((_, currentIndex) => currentIndex !== index),
    });
  };

  return (
    <div className="flex flex-col gap-4 rounded-lg border bg-muted/10 p-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="truncate text-sm font-semibold text-foreground">
            {filter.label || filter.id || 'Untitled filter'}
          </div>
          <div className="text-xs text-muted-foreground">{filter.id}</div>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="size-9"
          onClick={onRemove}
          aria-label={`Remove ${filter.label || filter.id}`}
        >
          <Trash2 className="size-4" />
        </Button>
      </div>

      <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-4">
        <EditorField label="Filter ID">
          <Input
            value={filter.id}
            onChange={(event) =>
              onChange({
                ...filter,
                id: slugify(event.target.value).replace(/-/g, '_'),
              })
            }
          />
        </EditorField>
        <EditorField label="Label">
          <Input
            value={filter.label}
            onChange={(event) =>
              onChange({ ...filter, label: event.target.value })
            }
          />
        </EditorField>
        <EditorField label="Control">
          <Select
            value={filter.type}
            onValueChange={(type) =>
              onChange({
                ...filter,
                type: type as ReportFilterType,
                default: defaultValueForFilterType(type as ReportFilterType),
                appliesTo: (filter.appliesTo ?? []).map((mapping) => ({
                  ...mapping,
                  op: defaultFilterOperator(type as ReportFilterType),
                })),
              })
            }
          >
            <SelectTrigger>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {FILTER_TYPE_OPTIONS.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </EditorField>
        <DefaultValueEditor
          filter={filter}
          onChange={(value) => onChange({ ...filter, default: value })}
        />
      </div>

      <div className="flex flex-col gap-3 rounded-md border bg-background p-3">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div>
            <div className="text-sm font-semibold text-foreground">
              Target mappings
            </div>
            <div className="text-xs text-muted-foreground">
              Map one filter to every block, dataset query, or field it should
              constrain.
            </div>
          </div>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={addMapping}
          >
            <Plus className="mr-2 size-4" />
            Add mapping
          </Button>
        </div>
        {mappings.length === 0 ? (
          <div className="rounded-md border border-dashed bg-muted/20 p-3 text-sm text-muted-foreground">
            No targets. The filter will render but will not constrain report
            data.
          </div>
        ) : (
          <div className="flex flex-col gap-2">
            {mappings.map((mapping, mappingIndex) => {
              const targetValue = mapping.blockId ?? ALL_TARGETS_SELECT_VALUE;
              const targetFields = getFilterMappingFields(
                definition,
                mapping.blockId,
                filterableBlocks,
                schemas
              );
              return (
                <div
                  key={`mapping-${mappingIndex}-${mapping.blockId ?? 'all'}`}
                  className="grid gap-2 rounded-md border p-3 md:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_12rem_40px]"
                >
                  <EditorField label="Applies to">
                    <Select
                      value={targetValue}
                      onValueChange={(value) =>
                        updateMappingTarget(mappingIndex, value)
                      }
                    >
                      <SelectTrigger>
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value={ALL_TARGETS_SELECT_VALUE}>
                          All compatible blocks and datasets
                        </SelectItem>
                        {filterableBlocks.map((block) => (
                          <SelectItem key={block.id} value={block.id}>
                            {block.title || block.id}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </EditorField>
                  <EditorField label="Field">
                    <Select
                      value={mapping.field || NONE_SELECT_VALUE}
                      disabled={targetFields.length === 0}
                      onValueChange={(field) =>
                        updateMapping(mappingIndex, { field })
                      }
                    >
                      <SelectTrigger>
                        <SelectValue placeholder="Select field" />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value={NONE_SELECT_VALUE} disabled>
                          Select field
                        </SelectItem>
                        {targetFields.map((field) => (
                          <SelectItem key={field} value={field}>
                            {field}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </EditorField>
                  <EditorField label="Operator">
                    <Select
                      value={mapping.op ?? defaultFilterOperator(filter.type)}
                      onValueChange={(op) =>
                        updateMapping(mappingIndex, { op })
                      }
                    >
                      <SelectTrigger>
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {FILTER_OPERATOR_OPTIONS.map((option) => (
                          <SelectItem key={option.value} value={option.value}>
                            {option.label}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </EditorField>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="mt-6 size-9"
                    onClick={() => removeMapping(mappingIndex)}
                    aria-label="Remove mapping"
                  >
                    <Trash2 className="size-4" />
                  </Button>
                </div>
              );
            })}
          </div>
        )}
      </div>

      <div className="grid gap-3 md:grid-cols-2">
        <label className="flex min-h-10 items-center gap-2 rounded-md border bg-background px-3 py-2 text-sm">
          <Checkbox
            checked={Boolean(filter.required)}
            onCheckedChange={(checked) =>
              onChange({ ...filter, required: Boolean(checked) })
            }
          />
          Required
        </label>
        <label className="flex min-h-10 items-center gap-2 rounded-md border bg-background px-3 py-2 text-sm">
          <Checkbox
            checked={Boolean(filter.strictWhenReferenced)}
            onCheckedChange={(checked) =>
              onChange({ ...filter, strictWhenReferenced: Boolean(checked) })
            }
          />
          Empty value hides dependent blocks
        </label>
      </div>

      {usesOptions(filter.type) && (
        <div className="flex flex-col gap-3 rounded-md border bg-background p-3">
          <div className="grid gap-3 md:grid-cols-[12rem_minmax(0,1fr)]">
            <EditorField label="Options source">
              <Select
                value={optionsSource}
                onValueChange={(source) =>
                  onChange({
                    ...filter,
                    options:
                      source === 'object_model'
                        ? {
                            source: 'object_model',
                            schema: optionsSchemaName,
                            valueField: valueField || optionFields[0] || '',
                            labelField:
                              labelField || valueField || optionFields[0] || '',
                            search: true,
                          }
                        : {
                            source: 'static',
                            values: filter.options?.values ?? [],
                          },
                  })
                }
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="static">Static values</SelectItem>
                  <SelectItem value="object_model">
                    Object Model lookup
                  </SelectItem>
                </SelectContent>
              </Select>
            </EditorField>
            {optionsSource === 'static' ? (
              <EditorField label="Static values">
                <Input
                  value={formatStaticFilterOptions(filter)}
                  placeholder="open, closed, pending"
                  onChange={(event) =>
                    onChange({
                      ...filter,
                      options: {
                        source: 'static',
                        values: parseStaticFilterOptions(event.target.value),
                      },
                    })
                  }
                />
              </EditorField>
            ) : (
              <div className="grid gap-3 md:grid-cols-3">
                <EditorField label="Options schema">
                  <Select
                    value={optionsSchemaName}
                    onValueChange={(schema) => {
                      const fields = getSchemaFieldNames(
                        schemas.find((candidate) => candidate.name === schema)
                      );
                      onChange({
                        ...filter,
                        options: {
                          ...filter.options,
                          source: 'object_model',
                          schema,
                          valueField: fields[0] ?? '',
                          labelField: fields[0] ?? '',
                          search: filter.options?.search ?? true,
                        },
                      });
                    }}
                  >
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
                </EditorField>
                <EditorField label="Value field">
                  <Select
                    value={valueField || NONE_SELECT_VALUE}
                    onValueChange={(field) =>
                      onChange({
                        ...filter,
                        options: {
                          ...filter.options,
                          source: 'object_model',
                          schema: optionsSchemaName,
                          valueField: field,
                          labelField: labelField || field,
                          search: filter.options?.search ?? true,
                        },
                      })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue placeholder="Field" />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value={NONE_SELECT_VALUE} disabled>
                        Select field
                      </SelectItem>
                      {optionFields.map((field) => (
                        <SelectItem key={field} value={field}>
                          {field}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </EditorField>
                <EditorField label="Label field">
                  <Select
                    value={labelField || NONE_SELECT_VALUE}
                    onValueChange={(field) =>
                      onChange({
                        ...filter,
                        options: {
                          ...filter.options,
                          source: 'object_model',
                          schema: optionsSchemaName,
                          valueField: valueField || field,
                          labelField: field,
                          search: filter.options?.search ?? true,
                        },
                      })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue placeholder="Field" />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value={NONE_SELECT_VALUE} disabled>
                        Select field
                      </SelectItem>
                      {optionFields.map((field) => (
                        <SelectItem key={field} value={field}>
                          {field}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </EditorField>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

function DefaultValueEditor({
  filter,
  onChange,
}: {
  filter: ReportFilterDefinition;
  onChange: (value: unknown) => void;
}) {
  if (filter.type === 'checkbox') {
    return (
      <EditorField label="Default">
        <label className="flex min-h-10 items-center gap-2 rounded-md border bg-background px-3 py-2 text-sm">
          <Checkbox
            checked={Boolean(filter.default)}
            onCheckedChange={(checked) => onChange(Boolean(checked))}
          />
          Checked
        </label>
      </EditorField>
    );
  }

  if (filter.type === 'time_range') {
    return (
      <EditorField label="Default">
        <Select
          value={String(filter.default ?? 'last_30_days')}
          onValueChange={onChange}
        >
          <SelectTrigger>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="today">Today</SelectItem>
            <SelectItem value="yesterday">Yesterday</SelectItem>
            <SelectItem value="last_7_days">Last 7 days</SelectItem>
            <SelectItem value="last_30_days">Last 30 days</SelectItem>
            <SelectItem value="this_month">This month</SelectItem>
            <SelectItem value="last_month">Last month</SelectItem>
            <SelectItem value="year_to_date">Year to date</SelectItem>
          </SelectContent>
        </Select>
      </EditorField>
    );
  }

  if (filter.type === 'multi_select') {
    return (
      <EditorField label="Default values">
        <Input
          value={Array.isArray(filter.default) ? filter.default.join(', ') : ''}
          placeholder="open, pending"
          onChange={(event) =>
            onChange(
              event.target.value
                .split(',')
                .map((part) => part.trim())
                .filter(Boolean)
            )
          }
        />
      </EditorField>
    );
  }

  return (
    <EditorField label="Default">
      <Input
        value={String(filter.default ?? '')}
        onChange={(event) => onChange(event.target.value)}
      />
    </EditorField>
  );
}

function ReportPreviewPanel({ definition }: { definition: ReportDefinition }) {
  const [filters, setFilters] = useState<Record<string, unknown>>(() =>
    defaultReportFilterValues(definition)
  );

  useEffect(() => {
    setFilters(defaultReportFilterValues(definition));
  }, [definition.filters]);

  const previewDefinition = useMemo(
    () => createPreviewDefinition(definition),
    [definition]
  );
  const visibleBlocks = useMemo(
    () => getPreviewBlocks(previewDefinition, filters),
    [filters, previewDefinition]
  );
  const previewRequest = useMemo(
    () => ({
      definition: previewDefinition,
      filters,
      blocks: visibleBlocks.map((block) => ({
        id: block.id,
        page:
          block.type === 'table'
            ? {
                offset: 0,
                size: block.table?.pagination?.defaultPageSize ?? 50,
              }
            : undefined,
        sort: block.table?.defaultSort ?? [],
      })),
      timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
    }),
    [filters, previewDefinition, visibleBlocks]
  );
  const { data, isFetching, isError, error, refetch } = useReportPreview(
    previewRequest,
    visibleBlocks.length > 0
  );

  return (
    <section className="flex flex-col gap-4 rounded-lg border bg-background p-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-center gap-2">
          <h2 className="text-base font-semibold text-foreground">Preview</h2>
          <Badge variant="secondary">{visibleBlocks.length} blocks</Badge>
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={isFetching}
          onClick={() => refetch()}
        >
          <RefreshCw className="mr-2 size-4" />
          Refresh
        </Button>
      </div>

      <ReportFilterBar
        definition={previewDefinition}
        values={filters}
        onChange={(filterId, value) =>
          setFilters((current) => ({ ...current, [filterId]: value }))
        }
      />

      {isError && (
        <Alert variant="destructive">
          <AlertTriangle className="size-4" />
          <AlertTitle>Preview failed</AlertTitle>
          <AlertDescription>{error.message}</AlertDescription>
        </Alert>
      )}

      {visibleBlocks.length === 0 ? (
        <div className="rounded-lg border border-dashed bg-muted/20 p-6 text-sm text-muted-foreground">
          Add a block to preview this report.
        </div>
      ) : (
        <div className={isFetching ? 'opacity-60' : undefined}>
          <ReportRenderer
            reportId="preview"
            definition={previewDefinition}
            renderResponse={data}
            filters={filters}
          />
        </div>
      )}
    </section>
  );
}

function ReportValidationPanel({
  localErrors,
  serverErrors,
  serverWarnings,
  isValid,
  isPending,
  onValidate,
}: {
  localErrors: string[];
  serverErrors: ReportValidationIssue[];
  serverWarnings: ReportValidationIssue[];
  isValid: boolean | undefined;
  isPending: boolean;
  onValidate: () => void | Promise<void>;
}) {
  return (
    <section className="flex flex-col gap-4 rounded-lg border bg-background p-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-center gap-2">
          <h2 className="text-base font-semibold text-foreground">
            Validation
          </h2>
          {isValid === true && localErrors.length === 0 ? (
            <Badge variant="success">Valid</Badge>
          ) : isValid === false || localErrors.length > 0 ? (
            <Badge variant="destructive">Needs attention</Badge>
          ) : (
            <Badge variant="secondary">Not checked</Badge>
          )}
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={isPending}
          onClick={onValidate}
        >
          {isPending ? (
            <RefreshCw className="mr-2 size-4 animate-spin" />
          ) : (
            <CheckCircle2 className="mr-2 size-4" />
          )}
          Validate
        </Button>
      </div>

      {localErrors.length === 0 &&
      serverErrors.length === 0 &&
      serverWarnings.length === 0 &&
      isValid === true ? (
        <Alert>
          <CheckCircle2 className="size-4" />
          <AlertTitle>Ready to save</AlertTitle>
          <AlertDescription>
            The report definition passed local and server validation.
          </AlertDescription>
        </Alert>
      ) : (
        <div className="flex flex-col gap-3">
          {localErrors.length > 0 && (
            <ValidationIssueList
              title="Local checks"
              issues={localErrors.map((message) => ({
                path: '$',
                code: 'LOCAL_VALIDATION',
                message,
              }))}
            />
          )}
          {serverErrors.length > 0 && (
            <ValidationIssueList title="Server errors" issues={serverErrors} />
          )}
          {serverWarnings.length > 0 && (
            <ValidationIssueList
              title="Server warnings"
              issues={serverWarnings}
            />
          )}
          {localErrors.length === 0 &&
            serverErrors.length === 0 &&
            serverWarnings.length === 0 && (
              <div className="rounded-lg border border-dashed bg-muted/20 p-6 text-sm text-muted-foreground">
                Run validation to check report syntax, schema references,
                filters, workflows, and block configuration.
              </div>
            )}
        </div>
      )}
    </section>
  );
}

function ValidationIssueList({
  title,
  issues,
}: {
  title: string;
  issues: ReportValidationIssue[];
}) {
  return (
    <div className="rounded-lg border bg-muted/10 p-3">
      <div className="mb-2 text-sm font-semibold text-foreground">{title}</div>
      <div className="flex flex-col gap-2">
        {issues.map((issue, index) => (
          <div
            key={`${issue.code}-${issue.path}-${index}`}
            className="rounded-md border bg-background p-3 text-sm"
          >
            <div className="flex flex-wrap items-center gap-2">
              <Badge variant="outline">{issue.code}</Badge>
              <span className="text-xs text-muted-foreground">
                {issue.path}
              </span>
            </div>
            <p className="mt-2 text-foreground">{issue.message}</p>
            {issue.hint && (
              <p className="mt-1 text-xs text-muted-foreground">{issue.hint}</p>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}

function ReportDatasetsEditor({
  definition,
  schemas,
  selectedSchema,
  onChange,
}: {
  definition: ReportDefinition;
  schemas: Schema[];
  selectedSchema: string;
  onChange: (definition: ReportDefinition) => void;
}) {
  const datasets = definition.datasets ?? [];

  const updateDatasets = (
    nextDatasets: ReportDatasetDefinition[],
    rename?: { previousId: string; nextId: string }
  ) => {
    const renamedBlocks =
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
    const nextDatasetById = new Map(
      nextDatasets.map((dataset) => [dataset.id, dataset])
    );
    const blocks = renamedBlocks.map((block) => {
      if (!block.dataset) return block;
      const dataset = nextDatasetById.get(block.dataset.id);
      return dataset ? reconcileDatasetBlock(block, dataset) : block;
    });
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
    const schema =
      schemas.find((candidate) => candidate.name === selectedSchema) ??
      schemas[0];
    if (!schema) return;
    const dataset = createDefaultDataset(schema, datasets);
    updateDatasets([...datasets, dataset]);
  };

  const removeDataset = (index: number) => {
    const removedDatasetId = datasets[index]?.id;
    const nextDatasets = datasets.filter(
      (_, currentIndex) => currentIndex !== index
    );
    if (!removedDatasetId) {
      updateDatasets(nextDatasets);
      return;
    }

    const removedBlockIds = new Set(
      definition.blocks
        .filter((block) => block.dataset?.id === removedDatasetId)
        .map((block) => block.id)
    );
    const blocks = definition.blocks.filter(
      (block) => !removedBlockIds.has(block.id)
    );
    onChange({
      ...definition,
      datasets: nextDatasets,
      blocks,
      layout: removeLayoutBlockReferences(definition.layout, removedBlockIds),
    });
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
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={schemas.length === 0}
          onClick={addDataset}
        >
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

  const updateSourceSchema = (schemaName: string) => {
    const schema = schemas.find((candidate) => candidate.name === schemaName);
    const nextDataset = createDefaultDataset(schema, []);
    onChange({
      ...nextDataset,
      id: dataset.id,
      label: dataset.label,
      source: {
        ...dataset.source,
        schema: schemaName,
      },
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
            onValueChange={updateSourceSchema}
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
                timeDimension: field === NONE_SELECT_VALUE ? undefined : field,
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
                  <Input
                    value={dimension.field}
                    readOnly
                    className="bg-muted/40"
                  />
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
                      <SelectItem value={NONE_SELECT_VALUE}>
                        No format
                      </SelectItem>
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
                        field: field === NONE_SELECT_VALUE ? undefined : field,
                      })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value={NONE_SELECT_VALUE}>
                        No field
                      </SelectItem>
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
    fields.find((field) =>
      /date|day|month|year|time|created|updated/i.test(field)
    ) ?? undefined;
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

function removeLayoutBlockReferences(
  layout: ReportDefinition['layout'],
  removedBlockIds: Set<string>
): ReportDefinition['layout'] {
  if (!layout) return layout;
  return layout
    .map((node) => removeLayoutBlockReference(node, removedBlockIds))
    .filter((node): node is NonNullable<ReportDefinition['layout']>[number] =>
      Boolean(node)
    );
}

function removeLayoutBlockReference(
  node: NonNullable<ReportDefinition['layout']>[number],
  removedBlockIds: Set<string>
): NonNullable<ReportDefinition['layout']>[number] | null {
  if (node.type === 'block') {
    return removedBlockIds.has(node.blockId) ? null : node;
  }

  if (node.type === 'metric_row') {
    const blocks = node.blocks.filter(
      (blockId) => !removedBlockIds.has(blockId)
    );
    return blocks.length === 0 ? null : { ...node, blocks };
  }

  if (node.type === 'section') {
    return {
      ...node,
      children: removeLayoutBlockReferences(node.children, removedBlockIds),
    };
  }

  if (node.type === 'columns') {
    return {
      ...node,
      columns: node.columns.map((column) => ({
        ...column,
        children: removeLayoutBlockReferences(column.children, removedBlockIds),
      })),
    };
  }

  if (node.type === 'grid') {
    const items = node.items.filter(
      (item) => !removedBlockIds.has(item.blockId)
    );
    return items.length === 0 ? null : { ...node, items };
  }

  return node;
}

function inferReportPrimarySchema(definition: ReportDefinition): string {
  return (
    definition.datasets?.find((dataset) => dataset.source?.schema)?.source
      .schema ??
    definition.blocks.find((block) => block.source?.schema)?.source.schema ??
    ''
  );
}

function inferDatasetFieldType(field: string): ReportDatasetFieldType {
  if (/date|day|month|year|_at$|time/i.test(field)) return 'date';
  if (/is_|has_|enabled|active/i.test(field)) return 'boolean';
  if (/amount|price|cost|total|qty|quantity|count|number|value/i.test(field)) {
    return 'number';
  }
  return 'string';
}

function inferDatasetFormat(field: string): ReportDatasetValueFormat {
  if (/date|day|month|year|_at$|time/i.test(field)) return 'date';
  if (/amount|price|cost/i.test(field)) return 'currency';
  if (/qty|quantity|count|number|total|value/i.test(field)) return 'number';
  return 'string';
}

function createDefaultReportFilter(
  definition: ReportDefinition,
  schemas: Schema[]
): ReportFilterDefinition {
  const block = definition.blocks.find((candidate) =>
    canMapFilterToBlock(definition, candidate.id, schemas)
  );
  const fields = block
    ? getFilterFieldNames(definition, block.id, schemas)
    : [];
  const field =
    fields.find((candidate) => /status|state|type|category/i.test(candidate)) ??
    fields[0] ??
    'id';
  const id = uniqueReportFilterId(definition.filters, field);

  return {
    id,
    label: humanizeFieldName(field),
    type: 'select',
    default: '',
    required: false,
    strictWhenReferenced: false,
    options: { source: 'static', values: [] },
    appliesTo: block ? [{ blockId: block.id, field, op: 'eq' }] : [],
  };
}

function normalizeFilterForType(
  filter: ReportFilterDefinition
): ReportFilterDefinition {
  if (!usesOptions(filter.type) && filter.options) {
    const { options: _options, ...rest } = filter;
    return rest;
  }

  if (usesOptions(filter.type) && !filter.options) {
    return { ...filter, options: { source: 'static', values: [] } };
  }

  return filter;
}

function defaultValueForFilterType(type: ReportFilterType): unknown {
  if (type === 'multi_select') return [];
  if (type === 'checkbox') return false;
  if (type === 'time_range') return 'last_30_days';
  if (type === 'number_range') return {};
  return '';
}

function defaultFilterOperator(type: ReportFilterType): string {
  if (type === 'multi_select') return 'in';
  if (type === 'time_range' || type === 'number_range') return 'between';
  if (type === 'search') return 'search';
  if (type === 'text') return 'contains';
  return 'eq';
}

function usesOptions(type: ReportFilterType): boolean {
  return type === 'select' || type === 'multi_select' || type === 'radio';
}

function uniqueReportFilterId(
  filters: ReportFilterDefinition[],
  seed: string
): string {
  const existingIds = new Set(filters.map((filter) => filter.id));
  const base = slugify(seed || 'filter').replace(/-/g, '_') || 'filter';
  let candidate = base.endsWith('_filter') ? base : `${base}_filter`;
  let suffix = 1;
  while (existingIds.has(candidate)) {
    suffix += 1;
    candidate = `${base}_filter_${suffix}`;
  }
  return candidate;
}

function canMapFilterToBlock(
  definition: ReportDefinition,
  blockId: string,
  schemas: Schema[]
): boolean {
  return getFilterFieldNames(definition, blockId, schemas).length > 0;
}

function getFilterFieldNames(
  definition: ReportDefinition,
  blockId: string,
  schemas: Schema[]
): string[] {
  const block = definition.blocks.find((candidate) => candidate.id === blockId);
  if (!block) return [];

  const schemaName = getBlockSourceSchema(definition, blockId);
  const schema = schemas.find((candidate) => candidate.name === schemaName);
  const schemaFields = getSchemaFieldNamesWithSystem(schema);
  if (schemaFields.length > 0) return schemaFields;

  const configuredFields = [
    ...(block.table?.columns ?? []).map((column) => column.field),
    ...(block.source.groupBy ?? []),
    ...(block.source.aggregates ?? []).flatMap((aggregate) =>
      aggregate.field ? [aggregate.field] : []
    ),
  ];
  return Array.from(new Set(configuredFields)).filter(Boolean);
}

function getFilterMappingFields(
  definition: ReportDefinition,
  blockId: string | undefined,
  filterableBlocks: ReportDefinition['blocks'],
  schemas: Schema[]
): string[] {
  if (blockId) {
    return getFilterFieldNames(definition, blockId, schemas);
  }
  return getGlobalFilterFieldNames(definition, filterableBlocks, schemas);
}

function getGlobalFilterFieldNames(
  definition: ReportDefinition,
  filterableBlocks: ReportDefinition['blocks'],
  schemas: Schema[]
): string[] {
  const fields = [
    ...filterableBlocks.flatMap((block) =>
      getFilterFieldNames(definition, block.id, schemas)
    ),
    ...(definition.datasets ?? []).flatMap((dataset) => [
      ...dataset.dimensions.map((dimension) => dimension.field),
      ...(dataset.timeDimension ? [dataset.timeDimension] : []),
      ...dataset.measures.flatMap((measure) =>
        measure.field ? [measure.field] : []
      ),
    ]),
  ];
  return Array.from(new Set(fields)).filter(Boolean);
}

function getBlockSourceSchema(
  definition: ReportDefinition,
  blockId: string
): string | undefined {
  const block = definition.blocks.find((candidate) => candidate.id === blockId);
  if (!block) return undefined;
  if (block.dataset) {
    return definition.datasets?.find(
      (dataset) => dataset.id === block.dataset?.id
    )?.source.schema;
  }
  return block.source?.schema || undefined;
}

function getSchemaFieldNamesWithSystem(schema: Schema | undefined): string[] {
  const fields = (schema?.columns ?? [])
    .map((column) => column.name)
    .filter(Boolean);
  return Array.from(new Set(['id', ...fields, 'createdAt', 'updatedAt']));
}

function formatStaticFilterOptions(filter: ReportFilterDefinition): string {
  return (
    filter.options?.values?.map((option) => String(option.value)).join(', ') ??
    ''
  );
}

function parseStaticFilterOptions(value: string) {
  return value
    .split(',')
    .map((part) => part.trim())
    .filter(Boolean)
    .map((part) => ({ label: humanizeFieldName(part), value: part }));
}

function defaultReportFilterValues(definition: ReportDefinition) {
  return Object.fromEntries(
    definition.filters.map((filter) => [
      filter.id,
      getFilterDefaultValue(filter),
    ])
  );
}

function createPreviewDefinition(
  definition: ReportDefinition
): ReportDefinition {
  return {
    ...definition,
    blocks: definition.blocks.map(sanitizePreviewBlock),
  };
}

function sanitizePreviewBlock(
  block: ReportBlockDefinition
): ReportBlockDefinition {
  if (block.type === 'actions') {
    return {
      id: block.id,
      type: 'markdown',
      title: block.title,
      lazy: false,
      source: { schema: '', mode: 'filter' },
      markdown: {
        content:
          '_Workflow action blocks are hidden in editor preview to avoid accidental execution._',
      },
      filters: [],
      showWhen: block.showWhen,
    };
  }

  return {
    ...block,
    lazy: false,
    actions: undefined,
    interactions: [],
    table: block.table
      ? {
          ...block.table,
          selectable: false,
          actions: [],
          columns: block.table.columns?.map((column) => ({
            ...column,
            editable: false,
            workflowAction: undefined,
            interactionButtons: [],
          })),
        }
      : block.table,
    card: block.card ? sanitizePreviewCard(block.card) : block.card,
  };
}

function sanitizePreviewCard(card: ReportCardConfig): ReportCardConfig {
  return {
    ...card,
    groups: card.groups.map((group) => ({
      ...group,
      fields: group.fields.map((field) => ({
        ...field,
        editable: false,
        workflowAction: undefined,
        subcard: field.subcard
          ? sanitizePreviewCard(field.subcard)
          : field.subcard,
      })),
    })),
  };
}

function getPreviewBlocks(
  definition: ReportDefinition,
  filters: Record<string, unknown>
) {
  const layout = getActiveReportLayout(definition);
  const visibleLayoutBlockIds = new Set(
    layout.length > 0 ? extractLayoutBlockReferences(layout) : []
  );

  return definition.blocks.filter((block) => {
    if (!isVisibleByShowWhen(block.showWhen, filters)) return false;
    return layout.length === 0 || visibleLayoutBlockIds.has(block.id);
  });
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
  const declaredBlockIds = new Set(definition.blocks.map((block) => block.id));
  const datasetIds = new Set<string>();
  const filterIds = new Set<string>();

  for (const filter of definition.filters ?? []) {
    if (!filter.id.trim()) {
      errors.push('Every report filter needs an ID.');
      continue;
    }
    if (filterIds.has(filter.id)) {
      errors.push(`Duplicate report filter ID: ${filter.id}`);
    }
    filterIds.add(filter.id);
    if (!filter.label.trim()) {
      errors.push(`Filter "${filter.id}" needs a label.`);
    }
    for (const mapping of filter.appliesTo ?? []) {
      if (!mapping.field.trim()) {
        errors.push(`Filter "${filter.id}" has a mapping without a field.`);
      }
      if (mapping.blockId && !declaredBlockIds.has(mapping.blockId)) {
        errors.push(
          `Filter "${filter.id}" maps to unknown block: ${mapping.blockId}`
        );
      }
    }
  }

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
    const sourceKind = block.source.kind ?? 'object_model';
    const isStaticMarkdown =
      block.type === 'markdown' &&
      !block.dataset &&
      !block.source.schema.trim();
    if (block.type === 'markdown' && !block.markdown?.content) {
      errors.push(`Block "${block.id}" needs markdown.content.`);
    }
    if (
      !block.dataset &&
      !isStaticMarkdown &&
      sourceKind === 'object_model' &&
      !block.source.schema.trim()
    ) {
      errors.push(`Block "${block.id}" needs a schema.`);
    }
    if (!block.dataset && sourceKind === 'workflow_runtime') {
      if (!block.source.workflowId?.trim()) {
        errors.push(`Block "${block.id}" needs a workflow ID.`);
      }
      if (!block.source.entity) {
        errors.push(`Block "${block.id}" needs a workflow runtime entity.`);
      }
    }
    if (block.dataset && !datasetIds.has(block.dataset.id)) {
      errors.push(
        `Block "${block.id}" references unknown dataset: ${block.dataset.id}`
      );
    }
    for (const datasetFilter of block.dataset?.datasetFilters ?? []) {
      if (!datasetFilter.field.trim()) {
        errors.push(
          `Block "${block.id}" has a dataset filter without a field.`
        );
      }
    }
    if (!block.dataset && sourceKind === 'object_model') {
      const joinAliases = new Set<string>();
      for (const join of block.source.join ?? []) {
        if (!join.schema.trim()) {
          errors.push(`Block "${block.id}" has a join without a schema.`);
        }
        if (!join.parentField.trim()) {
          errors.push(`Block "${block.id}" has a join without a parent field.`);
        }
        if (!join.field.trim()) {
          errors.push(`Block "${block.id}" has a join without a joined field.`);
        }
        const alias = join.alias?.trim() || join.schema;
        if (joinAliases.has(alias)) {
          errors.push(`Block "${block.id}" has duplicate join alias: ${alias}`);
        }
        joinAliases.add(alias);
      }
    }
  }

  for (const blockId of extractLayoutBlockReferences(definition.layout)) {
    if (!blockIds.has(blockId)) {
      errors.push(`Layout references unknown block: ${blockId}`);
    }
  }

  return errors;
}
