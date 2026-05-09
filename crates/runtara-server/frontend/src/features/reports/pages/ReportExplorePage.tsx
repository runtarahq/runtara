import { useEffect, useMemo, useState } from 'react';
import { Link, useNavigate, useParams, useSearchParams } from 'react-router';
import {
  ArrowLeft,
  Calendar,
  Copy,
  Filter,
  Hash,
  Plus,
  RotateCcw,
  Save,
  Search,
  Sigma,
  X,
} from 'lucide-react';
import { Badge } from '@/shared/components/ui/badge';
import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { TileList, TilesPage } from '@/shared/components/tiles-page';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { ReportDeleteButton } from '../components/ReportDeleteButton';
import { ReportFilterBar } from '../components/ReportFilterBar';
import { ChartBlock } from '../components/blocks/ChartBlock';
import { MetricBlock } from '../components/blocks/MetricBlock';
import { TableBlock } from '../components/blocks/TableBlock';
import {
  useReport,
  useReportDatasetQuery,
  useUpdateReport,
} from '../hooks/useReports';
import {
  ReportBlockDefinition,
  ReportChartKind,
  ReportDatasetDefinition,
  ReportDatasetFilterRequest,
  ReportDatasetQueryColumn,
  ReportDatasetQueryRequest,
  ReportDatasetQueryResponse,
  ReportDefinition,
  ReportOrderBy,
  ReportTableColumn,
} from '../types';
import {
  decodeFilterValue,
  encodeFilterValue,
  getFilterDefaultValue,
  humanizeFieldName,
  slugify,
} from '../utils';

type ExploreVizType = 'table' | 'metric' | ReportChartKind;

type ExploreFilter = ReportDatasetFilterRequest & {
  id: string;
};

type ExploreState = {
  datasetId: string;
  dimensions: string[];
  measures: string[];
  filters: ExploreFilter[];
  sort: ReportOrderBy[];
  vizType: ExploreVizType;
  preferredVizType: ExploreVizType;
  limit: number;
  page: {
    offset: number;
    size: number;
  };
  search: string;
  blockTitle: string;
};

const DEFAULT_PAGE_SIZE = 50;
const DEFAULT_LIMIT = 100;
const VIZ_TYPES: Array<{ value: ExploreVizType; label: string }> = [
  { value: 'table', label: 'Table' },
  { value: 'metric', label: 'Metric' },
  { value: 'bar', label: 'Bar chart' },
  { value: 'line', label: 'Line chart' },
  { value: 'area', label: 'Area chart' },
  { value: 'pie', label: 'Pie chart' },
  { value: 'donut', label: 'Donut chart' },
];

export function ReportExplorePage() {
  const { reportId } = useParams();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const { data: report, isFetching, isError, error } = useReport(reportId);
  const updateReport = useUpdateReport();
  const [state, setState] = useState<ExploreState | null>(null);
  const [stateKey, setStateKey] = useState('');
  const [fieldSearch, setFieldSearch] = useState('');
  const [draftFilterField, setDraftFilterField] = useState('');
  const [draftFilterOp, setDraftFilterOp] = useState('eq');
  const [draftFilterValue, setDraftFilterValue] = useState('');
  const [savedMessage, setSavedMessage] = useState<string | null>(null);
  const [copyMessage, setCopyMessage] = useState<string | null>(null);

  usePageTitle(report ? `${report.name} Explore` : 'Explore');

  const datasets = useMemo(
    () => (report?.definition.datasets ?? []).filter(isExplorableDataset),
    [report?.definition.datasets]
  );

  const reportFilters = useMemo(() => {
    if (!report) return {};
    return Object.fromEntries(
      report.definition.filters.map((filter) => [
        filter.id,
        decodeFilterValue(filter, searchParams.get(filter.id)),
      ])
    );
  }, [report, searchParams]);

  useEffect(() => {
    if (!report || datasets.length === 0) return;
    const nextStateKey = `${report.id}:${searchParams.get('block') ?? ''}`;
    if (state && stateKey === nextStateKey) return;
    setState(initialExploreState(report.definition, datasets, searchParams));
    setStateKey(nextStateKey);
  }, [datasets, report, searchParams, state, stateKey]);

  const dataset = useMemo(
    () => datasets.find((candidate) => candidate.id === state?.datasetId),
    [datasets, state?.datasetId]
  );

  const selectedMeasureFields = useMemo(
    () => new Set(state?.measures ?? []),
    [state?.measures]
  );
  const selectedDimensionFields = useMemo(
    () => new Set(state?.dimensions ?? []),
    [state?.dimensions]
  );

  const queryRequest = useMemo<ReportDatasetQueryRequest | undefined>(() => {
    if (!dataset || !state || state.measures.length === 0) return undefined;
    return {
      filters: reportFilters,
      datasetFilters: state.filters.map(({ field, op, value }) => ({
        field,
        op,
        value,
      })),
      dimensions: state.dimensions,
      measures: state.measures,
      orderBy: state.sort,
      limit: state.limit,
      search:
        state.search.trim().length > 0 && state.dimensions.length > 0
          ? { query: state.search.trim(), fields: state.dimensions }
          : undefined,
      page: state.vizType === 'metric' ? { offset: 0, size: 1 } : state.page,
      timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
    };
  }, [dataset, reportFilters, state]);

  const query = useReportDatasetQuery(
    reportId,
    dataset?.id,
    queryRequest,
    Boolean(dataset && queryRequest)
  );

  const savedBlock = useMemo(() => {
    if (!report || !dataset || !state) return null;
    return buildSavedBlock(report.definition, dataset, state);
  }, [dataset, report, state]);

  const sourceBlockId = searchParams.get('block');

  const handleReportFilterChanges = (updates: Record<string, unknown>) => {
    setSearchParams(
      (currentParams) => {
        const nextParams = new URLSearchParams(currentParams);
        for (const [filterId, value] of Object.entries(updates)) {
          const filter = report?.definition.filters.find(
            (candidate) => candidate.id === filterId
          );
          const defaultValue = filter
            ? getFilterDefaultValue(filter)
            : undefined;
          if (
            isEmptyFilterValue(value) ||
            JSON.stringify(value) === JSON.stringify(defaultValue)
          ) {
            nextParams.delete(filterId);
          } else {
            nextParams.set(filterId, encodeFilterValue(value));
          }
        }
        return nextParams;
      },
      { replace: true }
    );
    resetPage();
  };

  const updateState = (updater: (current: ExploreState) => ExploreState) => {
    setSavedMessage(null);
    setState((current) => (current ? updater(current) : current));
  };

  const resetPage = () => {
    setState((current) =>
      current ? { ...current, page: { ...current.page, offset: 0 } } : current
    );
  };

  const handleDatasetChange = (datasetId: string) => {
    const nextDataset = datasets.find(
      (candidate) => candidate.id === datasetId
    );
    if (!report || !nextDataset) return;
    setState(defaultExploreState(nextDataset, report.definition.blocks));
    setDraftFilterField('');
    setDraftFilterValue('');
  };

  const addDimension = (field: string) => {
    updateState((current) => {
      const dimensions = current.dimensions.includes(field)
        ? current.dimensions
        : [...current.dimensions, field];
      return {
        ...current,
        dimensions,
        vizType: preserveVizType(
          dataset,
          current,
          dimensions,
          current.measures
        ),
        page: { ...current.page, offset: 0 },
      };
    });
  };

  const addMeasure = (id: string) => {
    updateState((current) => {
      const measures = current.measures.includes(id)
        ? current.measures
        : [...current.measures, id];
      return {
        ...current,
        measures,
        vizType: preserveVizType(
          dataset,
          current,
          current.dimensions,
          measures
        ),
        page: { ...current.page, offset: 0 },
      };
    });
  };

  const removeDimension = (field: string) => {
    updateState((current) => {
      const dimensions = current.dimensions.filter((item) => item !== field);
      return {
        ...current,
        dimensions,
        sort: current.sort.filter((sort) => sort.field !== field),
        vizType: preserveVizType(
          dataset,
          current,
          dimensions,
          current.measures
        ),
        page: { ...current.page, offset: 0 },
      };
    });
  };

  const removeMeasure = (id: string) => {
    updateState((current) => {
      const measures = current.measures.filter((item) => item !== id);
      return {
        ...current,
        measures,
        sort: current.sort.filter((sort) => sort.field !== id),
        vizType: preserveVizType(
          dataset,
          current,
          current.dimensions,
          measures
        ),
        page: { ...current.page, offset: 0 },
      };
    });
  };

  const addExploreFilter = () => {
    if (!draftFilterField || draftFilterValue.trim().length === 0) return;
    const value = parseFilterValue(dataset, draftFilterField, draftFilterValue);
    updateState((current) => ({
      ...current,
      filters: [
        ...current.filters,
        {
          id: `${draftFilterField}-${Date.now()}`,
          field: draftFilterField,
          op: draftFilterOp,
          value,
        },
      ],
      page: { ...current.page, offset: 0 },
    }));
    setDraftFilterValue('');
  };

  const removeExploreFilter = (filterId: string) => {
    updateState((current) => ({
      ...current,
      filters: current.filters.filter((filter) => filter.id !== filterId),
      page: { ...current.page, offset: 0 },
    }));
  };

  const addPointFilter = (
    datum: Record<string, unknown>,
    fieldOverride?: string
  ): boolean => {
    if (!state || !dataset || state.dimensions.length === 0) return false;
    const field =
      fieldOverride && state.dimensions.includes(fieldOverride)
        ? fieldOverride
        : state.dimensions[0];
    const value =
      typeof datum.field === 'string' &&
      datum.field === field &&
      'value' in datum
        ? datum.value
        : datum[field];
    if (value === undefined || value === null) return false;
    updateState((current) => ({
      ...current,
      filters: [
        ...current.filters,
        {
          id: `${field}-${Date.now()}`,
          field,
          op: 'eq',
          value,
        },
      ],
      page: { ...current.page, offset: 0 },
    }));
    return true;
  };

  const handleSaveBlock = async (mode: 'append' | 'replace' = 'append') => {
    if (!report || !savedBlock || !state) return;
    const nextBlock =
      mode === 'replace' && sourceBlockId
        ? {
            ...savedBlock,
            id: sourceBlockId,
            title: state.blockTitle || savedBlock.title,
          }
        : savedBlock;
    const nextDefinition =
      mode === 'replace' && sourceBlockId
        ? replaceBlockInDefinition(report.definition, sourceBlockId, nextBlock)
        : appendBlockToDefinition(report.definition, nextBlock);
    await updateReport.mutateAsync({
      id: report.id,
      data: {
        name: report.name,
        slug: report.slug,
        description: report.description,
        tags: report.tags,
        status: report.status,
        definition: nextDefinition,
      },
    });
    setSavedMessage(`Saved "${nextBlock.title ?? nextBlock.id}".`);
    navigate(buildReportPath(report.id, report.definition, searchParams));
  };

  const handleCopyDefinition = async () => {
    if (!savedBlock) return;
    await navigator.clipboard.writeText(JSON.stringify(savedBlock, null, 2));
    setCopyMessage('Copied dataset block JSON.');
  };

  if (isFetching) {
    return (
      <TilesPage kicker="Reports" title="Loading Explore">
        <TileList>
          <div className="h-96 animate-pulse rounded-lg bg-muted/30" />
        </TileList>
      </TilesPage>
    );
  }

  if (isError || !report) {
    return (
      <TilesPage kicker="Reports" title="Explore unavailable">
        <TileList>
          <div className="rounded-lg border bg-background p-6 text-sm text-muted-foreground">
            {error?.message ?? 'The report could not be loaded.'}
          </div>
        </TileList>
      </TilesPage>
    );
  }

  if (datasets.length === 0 || !state || !dataset) {
    return (
      <TilesPage
        kicker="Reports"
        title="Explore"
        action={
          <div className="flex w-full flex-col gap-2 sm:w-auto sm:flex-row">
            <Link to={`/reports/${report.id}`}>
              <Button variant="outline" className="h-11 rounded-full sm:px-5">
                <ArrowLeft className="mr-2 h-4 w-4" />
                Report
              </Button>
            </Link>
            <ReportDeleteButton
              reportId={report.id}
              reportName={report.name}
              className="h-11 rounded-full sm:px-5"
            />
          </div>
        }
      >
        <div className="rounded-lg border bg-background p-6 text-sm text-muted-foreground">
          This report does not expose a semantic dataset for Explore.
        </div>
      </TilesPage>
    );
  }

  const fields = filteredFields(dataset, fieldSearch);
  const vizOptions = VIZ_TYPES.map((item) => ({
    ...item,
    validity: vizValidity(item.value, dataset, state),
  }));
  const currentVizValidity = vizValidity(state.vizType, dataset, state);
  const columns = query.data?.columns ?? [];
  const blockPreview = buildPreviewBlock(dataset, state, columns);
  const canSaveBlock = Boolean(
    savedBlock &&
      currentVizValidity.valid &&
      !query.isFetching &&
      !query.isError &&
      query.data
  );
  const canReplaceBlock = Boolean(
    canSaveBlock &&
      sourceBlockId &&
      report.definition.blocks.some(
        (block) => block.id === sourceBlockId && block.dataset
      )
  );

  return (
    <TilesPage
      kicker="Reports / Explore"
      title={report.name}
      action={
        <div className="flex w-full flex-col gap-2 sm:w-auto sm:flex-row">
          <Link to={`/reports/${report.id}`}>
            <Button variant="outline" className="h-11 rounded-full sm:px-5">
              <ArrowLeft className="mr-2 h-4 w-4" />
              Report
            </Button>
          </Link>
          <ReportDeleteButton
            reportId={report.id}
            reportName={report.name}
            className="h-11 rounded-full sm:px-5"
          />
          <Button
            className="h-11 rounded-full sm:px-5"
            disabled={!canSaveBlock || updateReport.isPending}
            onClick={() =>
              handleSaveBlock(canReplaceBlock ? 'replace' : 'append')
            }
          >
            <Save className="mr-2 h-4 w-4" />
            {canReplaceBlock ? 'Update block' : 'Save as block'}
          </Button>
        </div>
      }
      contentClassName="pb-10"
    >
      <div className="mb-4 flex flex-col gap-3 rounded-lg border bg-background p-3">
        <div className="flex flex-wrap items-center gap-2 text-sm">
          <Badge variant="outline">Dataset: {dataset.label}</Badge>
          {state.dimensions.map((field) => (
            <Badge key={field} variant="secondary">
              {fieldLabel(dataset, field)}
            </Badge>
          ))}
          {state.measures.map((field) => (
            <Badge key={field} variant="default">
              {measureLabel(dataset, field)}
            </Badge>
          ))}
          {state.filters.map((filter) => (
            <Button
              key={filter.id}
              type="button"
              variant="outline"
              size="sm"
              className="h-7 rounded-full px-3 text-xs"
              onClick={() => removeExploreFilter(filter.id)}
            >
              Explore: {fieldLabel(dataset, filter.field)} {filter.op}{' '}
              {formatFilterValue(filter.value)}
              <X className="ml-2 h-3 w-3" />
            </Button>
          ))}
        </div>
        <ReportFilterBar
          reportId={report.id}
          definition={report.definition}
          values={reportFilters}
          onChange={(filterId, value) =>
            handleReportFilterChanges({ [filterId]: value })
          }
        />
      </div>

      <div className="grid gap-4 xl:grid-cols-[300px_minmax(0,1fr)_320px]">
        <aside className="order-1 min-w-0 rounded-lg border bg-background xl:order-1">
          <div className="border-b p-4">
            <Label>Dataset</Label>
            <Select value={state.datasetId} onValueChange={handleDatasetChange}>
              <SelectTrigger className="mt-2">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {datasets.map((candidate) => (
                  <SelectItem key={candidate.id} value={candidate.id}>
                    {candidate.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <div className="mt-3 rounded-md bg-muted/30 p-3 text-xs text-muted-foreground">
              <div className="font-medium text-foreground">{dataset.label}</div>
              <div>Source: {dataset.source.schema}</div>
              <div>Time: {dataset.timeDimension ?? 'None'}</div>
              <div>
                {visibleDimensions(dataset).length} dimensions,{' '}
                {visibleMeasures(dataset).length} measures
              </div>
            </div>
          </div>
          <div className="border-b p-4">
            <div className="relative">
              <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
              <Input
                value={fieldSearch}
                onChange={(event) => setFieldSearch(event.target.value)}
                placeholder="Search fields"
                className="pl-9"
              />
            </div>
          </div>
          <FieldGroup
            title="Dimensions"
            fields={fields.dimensions}
            selected={selectedDimensionFields}
            onAdd={addDimension}
            onRemove={removeDimension}
          />
          <FieldGroup
            title="Measures"
            fields={fields.measures}
            selected={selectedMeasureFields}
            onAdd={addMeasure}
            onRemove={removeMeasure}
          />
        </aside>

        <main className="order-3 min-w-0 space-y-4 xl:order-2">
          <section className="rounded-lg border bg-background p-4">
            <div className="mb-4 grid gap-3 lg:grid-cols-2">
              <Shelf
                title="Rows / Dimensions"
                empty="Add a dimension"
                values={state.dimensions.map((field) => ({
                  id: field,
                  label: fieldLabel(dataset, field),
                }))}
                onRemove={removeDimension}
              />
              <Shelf
                title="Measures"
                empty="Add a measure"
                values={state.measures.map((field) => ({
                  id: field,
                  label: measureLabel(dataset, field),
                }))}
                onRemove={removeMeasure}
              />
            </div>
            {query.isFetching ? (
              <div className="h-80 animate-pulse rounded-lg border bg-muted/30" />
            ) : query.isError ? (
              <div className="rounded-lg border border-destructive/30 bg-destructive/5 p-4 text-sm text-destructive">
                {query.error.message}
              </div>
            ) : state.measures.length === 0 ? (
              <div className="flex min-h-72 items-center justify-center rounded-lg border bg-muted/20 text-sm text-muted-foreground">
                Add at least one measure to run a dataset query.
              </div>
            ) : (
              <ExplorePreview
                reportId={reportId ?? ''}
                block={blockPreview}
                vizType={state.vizType}
                data={query.data}
                sort={state.sort}
                onSortChange={(sort) =>
                  updateState((current) => ({
                    ...current,
                    sort,
                    page: { ...current.page, offset: 0 },
                  }))
                }
                onPageChange={(offset, size) =>
                  updateState((current) => ({
                    ...current,
                    page: { offset, size },
                  }))
                }
                onDrillFilter={addPointFilter}
              />
            )}
          </section>
        </main>

        <aside className="order-2 min-w-0 space-y-4 rounded-lg border bg-background p-4 xl:order-3">
          <section className="space-y-3">
            <h2 className="text-sm font-semibold text-foreground">
              Visualization
            </h2>
            <Select
              value={state.vizType}
              onValueChange={(value) =>
                updateState((current) => ({
                  ...current,
                  vizType: value as ExploreVizType,
                  preferredVizType: value as ExploreVizType,
                  page: { ...current.page, offset: 0 },
                }))
              }
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {vizOptions.map((option) => (
                  <SelectItem
                    key={option.value}
                    value={option.value}
                    disabled={!option.validity.valid}
                  >
                    {option.label}
                    {!option.validity.valid
                      ? ` - ${option.validity.reason}`
                      : ''}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <div className="rounded-md bg-muted/30 p-3 text-xs text-muted-foreground">
              {currentVizValidity.valid
                ? chartRecommendation(dataset, state)
                : currentVizValidity.reason}
            </div>
          </section>

          <section className="space-y-3">
            <h2 className="text-sm font-semibold text-foreground">Sort</h2>
            <Select
              value={state.sort[0]?.field ?? 'none'}
              onValueChange={(field) =>
                updateState((current) => ({
                  ...current,
                  sort:
                    field === 'none'
                      ? []
                      : [
                          {
                            field,
                            direction: current.sort[0]?.direction ?? 'desc',
                          },
                        ],
                  page: { ...current.page, offset: 0 },
                }))
              }
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="none">No explicit sort</SelectItem>
                {[...state.dimensions, ...state.measures].map((field) => (
                  <SelectItem key={field} value={field}>
                    {outputFieldLabel(dataset, field)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Select
              value={state.sort[0]?.direction ?? 'desc'}
              onValueChange={(direction) =>
                updateState((current) => ({
                  ...current,
                  sort:
                    current.sort.length === 0
                      ? current.sort
                      : [{ ...current.sort[0], direction }],
                }))
              }
              disabled={state.sort.length === 0}
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="desc">Descending</SelectItem>
                <SelectItem value="asc">Ascending</SelectItem>
              </SelectContent>
            </Select>
            <div className="grid grid-cols-2 gap-2">
              <div className="space-y-1">
                <Label htmlFor="explore-limit">Limit</Label>
                <Input
                  id="explore-limit"
                  type="number"
                  min={1}
                  max={500}
                  value={state.limit}
                  onChange={(event) => {
                    const limit = clampNumber(
                      event.target.value,
                      1,
                      500,
                      DEFAULT_LIMIT
                    );
                    updateState((current) => ({
                      ...current,
                      limit,
                      page: {
                        offset: 0,
                        size: Math.min(current.page.size, limit),
                      },
                    }));
                  }}
                />
              </div>
              <div className="space-y-1">
                <Label htmlFor="explore-page-size">Page size</Label>
                <Input
                  id="explore-page-size"
                  type="number"
                  min={1}
                  max={200}
                  value={state.page.size}
                  onChange={(event) =>
                    updateState((current) => ({
                      ...current,
                      page: {
                        offset: 0,
                        size: clampNumber(
                          event.target.value,
                          1,
                          200,
                          DEFAULT_PAGE_SIZE
                        ),
                      },
                    }))
                  }
                />
              </div>
            </div>
          </section>

          <section className="space-y-3">
            <h2 className="text-sm font-semibold text-foreground">
              Explore filters
            </h2>
            <Select
              value={draftFilterField}
              onValueChange={(field) => {
                setDraftFilterField(field);
                setDraftFilterOp(defaultFilterOp(dataset, field));
              }}
            >
              <SelectTrigger>
                <SelectValue placeholder="Field" />
              </SelectTrigger>
              <SelectContent>
                {visibleDimensions(dataset).map((dimension) => (
                  <SelectItem key={dimension.field} value={dimension.field}>
                    {dimension.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Select value={draftFilterOp} onValueChange={setDraftFilterOp}>
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {filterOpsForField(dataset, draftFilterField).map((op) => (
                  <SelectItem key={op.value} value={op.value}>
                    {op.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <div className="flex gap-2">
              <Input
                value={draftFilterValue}
                onChange={(event) => setDraftFilterValue(event.target.value)}
                placeholder="Value"
                onKeyDown={(event) => {
                  if (event.key === 'Enter') addExploreFilter();
                }}
              />
              <Button
                type="button"
                variant="outline"
                size="icon"
                disabled={
                  !draftFilterField || draftFilterValue.trim().length === 0
                }
                onClick={addExploreFilter}
              >
                <Plus className="h-4 w-4" />
              </Button>
            </div>
            <div className="space-y-2 rounded-md border bg-muted/20 p-3">
              <div className="text-xs font-semibold uppercase text-muted-foreground">
                Active explore filters
              </div>
              {state.filters.length === 0 ? (
                <p className="text-xs text-muted-foreground">
                  No explore filters.
                </p>
              ) : (
                <div className="space-y-2">
                  {state.filters.map((filter) => (
                    <button
                      key={filter.id}
                      type="button"
                      className="flex w-full items-center gap-2 rounded-md border bg-background px-3 py-2 text-left text-sm transition-colors hover:bg-muted/40"
                      onClick={() => removeExploreFilter(filter.id)}
                    >
                      <span className="min-w-0 flex-1">
                        <span className="block truncate font-medium text-foreground">
                          {fieldLabel(dataset, filter.field)}
                        </span>
                        <span className="block truncate text-xs text-muted-foreground">
                          {filter.op} {formatFilterValue(filter.value)}
                        </span>
                      </span>
                      <X className="h-4 w-4 shrink-0 text-muted-foreground" />
                    </button>
                  ))}
                </div>
              )}
            </div>
            <Button
              type="button"
              variant="outline"
              className="w-full"
              onClick={() =>
                updateState((current) => ({
                  ...current,
                  filters: [],
                  page: { ...current.page, offset: 0 },
                }))
              }
            >
              <RotateCcw className="mr-2 h-4 w-4" />
              Reset explore filters
            </Button>
          </section>

          <section className="space-y-3">
            <h2 className="text-sm font-semibold text-foreground">Save</h2>
            <Input
              value={state.blockTitle}
              onChange={(event) =>
                updateState((current) => ({
                  ...current,
                  blockTitle: event.target.value,
                }))
              }
              placeholder="Block title"
            />
            <Button
              className="w-full"
              disabled={!canSaveBlock || updateReport.isPending}
              onClick={() =>
                handleSaveBlock(canReplaceBlock ? 'replace' : 'append')
              }
            >
              <Save className="mr-2 h-4 w-4" />
              {canReplaceBlock ? 'Update report block' : 'Save as report block'}
            </Button>
            {canReplaceBlock && (
              <Button
                type="button"
                variant="outline"
                className="w-full"
                disabled={!canSaveBlock || updateReport.isPending}
                onClick={() => handleSaveBlock('append')}
              >
                <Save className="mr-2 h-4 w-4" />
                Save as copy
              </Button>
            )}
            <Button
              type="button"
              variant="outline"
              className="w-full"
              disabled={!savedBlock}
              onClick={handleCopyDefinition}
            >
              <Copy className="mr-2 h-4 w-4" />
              Copy definition
            </Button>
            {(savedMessage || copyMessage) && (
              <p className="text-xs text-muted-foreground">
                {savedMessage ?? copyMessage}
              </p>
            )}
          </section>
        </aside>
      </div>
    </TilesPage>
  );
}

function ExplorePreview({
  reportId,
  block,
  vizType,
  data,
  sort,
  onSortChange,
  onPageChange,
  onDrillFilter,
}: {
  reportId: string;
  block: ReportBlockDefinition;
  vizType: ExploreVizType;
  data?: ReportDatasetQueryResponse | null;
  sort: ReportOrderBy[];
  onSortChange: (sort: ReportOrderBy[]) => void;
  onPageChange: (offset: number, size: number) => void;
  onDrillFilter: (datum: Record<string, unknown>, field?: string) => boolean;
}) {
  const result = datasetResultToBlockResult(block, data);
  const tableBlock = {
    ...block,
    type: 'table' as const,
    table: {
      columns: tableColumnsFromDataset(data?.columns ?? []),
      defaultSort: sort,
      pagination: {
        defaultPageSize: data?.page.size ?? DEFAULT_PAGE_SIZE,
        allowedPageSizes: [25, 50, 100],
      },
    },
  };

  if (vizType === 'metric') {
    return <MetricBlock block={block} result={result} />;
  }

  if (vizType === 'table') {
    return (
      <TableBlock
        reportId={reportId}
        block={tableBlock}
        result={result}
        sort={sort}
        filters={{}}
        blockFilters={{}}
        onSortChange={onSortChange}
        onPageChange={onPageChange}
        onRowClick={onDrillFilter}
        onCellClick={(cell) =>
          onDrillFilter(
            cell,
            typeof cell.field === 'string' ? cell.field : undefined
          )
        }
      />
    );
  }

  return (
    <div className="space-y-4">
      <ChartBlock
        block={block}
        result={result}
        onPointClick={(datum) => onDrillFilter(datum, block.chart?.x)}
      />
      <div>
        <h3 className="mb-2 text-sm font-medium text-foreground">
          Result table
        </h3>
        <TableBlock
          reportId={reportId}
          block={tableBlock}
          result={result}
          sort={sort}
          filters={{}}
          blockFilters={{}}
          onSortChange={onSortChange}
          onPageChange={onPageChange}
          onRowClick={onDrillFilter}
          onCellClick={(cell) =>
            onDrillFilter(
              cell,
              typeof cell.field === 'string' ? cell.field : undefined
            )
          }
        />
      </div>
    </div>
  );
}

function FieldGroup({
  title,
  fields,
  selected,
  onAdd,
  onRemove,
}: {
  title: string;
  fields: CatalogField[];
  selected: Set<string>;
  onAdd: (id: string) => void;
  onRemove: (id: string) => void;
}) {
  return (
    <div className="border-b p-4 last:border-b-0">
      <h2 className="mb-3 text-sm font-semibold text-foreground">{title}</h2>
      <div className="space-y-2">
        {fields.map((field) => {
          const isSelected = selected.has(field.id);
          const Icon =
            field.kind === 'measure'
              ? Sigma
              : field.type === 'date' || field.type === 'datetime'
                ? Calendar
                : field.type === 'number' || field.type === 'decimal'
                  ? Hash
                  : Filter;
          return (
            <button
              key={field.id}
              type="button"
              aria-pressed={isSelected}
              className={`flex w-full items-start gap-2 rounded-md border px-3 py-2 text-left transition-colors ${
                isSelected
                  ? 'border-primary bg-primary/5'
                  : 'border-input hover:bg-muted/40'
              }`}
              onClick={() =>
                isSelected ? onRemove(field.id) : onAdd(field.id)
              }
            >
              <Icon className="mt-0.5 h-4 w-4 shrink-0 text-muted-foreground" />
              <span className="min-w-0 flex-1">
                <span className="block text-sm font-medium text-foreground">
                  {field.label}
                </span>
                <span className="block truncate text-xs text-muted-foreground">
                  {field.id} · {field.format ?? field.type}
                </span>
              </span>
              {isSelected && (
                <Badge variant="secondary">
                  Added
                  <X className="ml-1 h-3 w-3" />
                </Badge>
              )}
            </button>
          );
        })}
        {fields.length === 0 && (
          <p className="text-xs text-muted-foreground">No matching fields.</p>
        )}
      </div>
    </div>
  );
}

function Shelf({
  title,
  empty,
  values,
  onRemove,
}: {
  title: string;
  empty: string;
  values: Array<{ id: string; label: string }>;
  onRemove: (id: string) => void;
}) {
  return (
    <div className="min-h-20 rounded-md border border-dashed bg-muted/20 p-3">
      <div className="mb-2 text-xs font-semibold uppercase text-muted-foreground">
        {title}
      </div>
      <div className="flex flex-wrap gap-2">
        {values.map((value) => (
          <Button
            key={value.id}
            type="button"
            variant="outline"
            size="sm"
            className="h-8 rounded-full px-3 text-xs"
            onClick={() => onRemove(value.id)}
          >
            {value.label}
            <X className="ml-2 h-3 w-3" />
          </Button>
        ))}
        {values.length === 0 && (
          <span className="text-sm text-muted-foreground">{empty}</span>
        )}
      </div>
    </div>
  );
}

type CatalogField = {
  id: string;
  label: string;
  type: string;
  format?: string;
  kind: 'dimension' | 'measure';
};

function initialExploreState(
  definition: { blocks: ReportBlockDefinition[] },
  datasets: ReportDatasetDefinition[],
  searchParams: URLSearchParams
): ExploreState {
  const blockId = searchParams.get('block');
  const block = definition.blocks.find(
    (candidate) => candidate.id === blockId && candidate.dataset
  );
  if (block?.dataset) {
    const dataset = datasets.find(
      (candidate) => candidate.id === block.dataset?.id
    );
    if (dataset) {
      const dimensions = block.dataset.dimensions ?? [];
      const measures = block.dataset.measures ?? [];
      return {
        datasetId: dataset.id,
        dimensions,
        measures,
        filters: (block.dataset.datasetFilters ?? []).map((filter, index) => ({
          ...filter,
          id: `${filter.field}-${index}`,
          op: filter.op ?? 'eq',
        })),
        sort: block.dataset.orderBy ?? [],
        vizType:
          block.type === 'chart'
            ? (block.chart?.kind ??
              recommendVizType(dataset, dimensions, measures))
            : block.type === 'metric'
              ? 'metric'
              : block.type === 'table'
                ? 'table'
                : recommendVizType(dataset, dimensions, measures),
        preferredVizType:
          block.type === 'chart'
            ? (block.chart?.kind ??
              recommendVizType(dataset, dimensions, measures))
            : block.type === 'metric'
              ? 'metric'
              : block.type === 'table'
                ? 'table'
                : recommendVizType(dataset, dimensions, measures),
        limit: block.dataset.limit ?? DEFAULT_LIMIT,
        page: { offset: 0, size: DEFAULT_PAGE_SIZE },
        search: '',
        blockTitle: block.title ?? dataset.label,
      };
    }
  }

  return defaultExploreState(datasets[0], definition.blocks);
}

function defaultExploreState(
  dataset: ReportDatasetDefinition,
  blocks: ReportBlockDefinition[]
): ExploreState {
  const dimensions = defaultDimensions(dataset);
  const measures = defaultMeasures(dataset);
  const vizType = recommendVizType(dataset, dimensions, measures);
  return {
    datasetId: dataset.id,
    dimensions,
    measures,
    filters: [],
    sort: defaultSort({ dimensions, measures }),
    vizType,
    preferredVizType: vizType,
    limit: DEFAULT_LIMIT,
    page: { offset: 0, size: DEFAULT_PAGE_SIZE },
    search: '',
    blockTitle: uniqueBlockTitle(
      blocks,
      defaultBlockTitle(dataset, dimensions, measures)
    ),
  };
}

function defaultDimensions(dataset: ReportDatasetDefinition): string[] {
  const dimensions = visibleDimensions(dataset);
  const preferred =
    dimensions.find((dimension) => /vendor/i.test(dimension.field)) ??
    dimensions.find((dimension) => dimension.field !== dataset.timeDimension) ??
    dimensions[0];
  return preferred ? [preferred.field] : [];
}

function defaultMeasures(dataset: ReportDatasetDefinition): string[] {
  const measures = visibleMeasures(dataset);
  const preferred =
    measures.find((measure) => /qty_total|total|sum/i.test(measure.id)) ??
    measures.find((measure) => measure.op !== 'count') ??
    measures[0];
  return preferred ? [preferred.id] : [];
}

function defaultSort(state: Pick<ExploreState, 'dimensions' | 'measures'>) {
  const field = state.measures[0] ?? state.dimensions[0];
  return field ? [{ field, direction: 'desc' }] : [];
}

function recommendVizType(
  dataset: ReportDatasetDefinition | undefined,
  dimensions: string[],
  measures: string[]
): ExploreVizType {
  if (measures.length === 1 && dimensions.length === 0) return 'metric';
  if (
    dataset?.timeDimension &&
    dimensions.length === 1 &&
    dimensions[0] === dataset.timeDimension
  ) {
    return 'line';
  }
  if (dimensions.length === 1 && measures.length >= 1) return 'bar';
  return 'table';
}

function preserveVizType(
  dataset: ReportDatasetDefinition | undefined,
  current: ExploreState,
  dimensions: string[],
  measures: string[]
): ExploreVizType {
  const preferredVizType = current.preferredVizType ?? current.vizType;
  if (
    dataset &&
    vizValidity(preferredVizType, dataset, {
      ...current,
      dimensions,
      measures,
    }).valid
  ) {
    return preferredVizType;
  }
  return recommendVizType(dataset, dimensions, measures);
}

function vizValidity(
  vizType: ExploreVizType,
  dataset: ReportDatasetDefinition,
  state: ExploreState
): { valid: boolean; reason?: string } {
  if (state.measures.length === 0) {
    return { valid: false, reason: 'requires a measure' };
  }
  if (vizType === 'metric') {
    return state.measures.length === 1 && state.dimensions.length === 0
      ? { valid: true }
      : { valid: false, reason: 'one measure, no dimensions' };
  }
  if (vizType === 'line' || vizType === 'area') {
    return state.dimensions.length === 1 &&
      state.dimensions[0] === dataset.timeDimension
      ? { valid: true }
      : { valid: false, reason: 'use the time dimension' };
  }
  if (vizType === 'pie' || vizType === 'donut') {
    return state.dimensions.length === 1 && state.measures.length === 1
      ? { valid: true }
      : { valid: false, reason: 'one dimension and one measure' };
  }
  if (vizType === 'bar') {
    return state.dimensions.length >= 1 && state.measures.length >= 1
      ? { valid: true }
      : { valid: false, reason: 'dimension plus measure' };
  }
  return { valid: true };
}

function chartRecommendation(
  dataset: ReportDatasetDefinition,
  state: ExploreState
): string {
  if (
    state.vizType ===
    recommendVizType(dataset, state.dimensions, state.measures)
  ) {
    return 'Recommended for the selected dataset fields.';
  }
  return 'Manual visualization override for this query.';
}

function defaultBlockTitle(
  dataset: ReportDatasetDefinition,
  dimensions: string[],
  measures: string[]
): string {
  const measure = measures[0]
    ? measureLabel(dataset, measures[0])
    : dataset.label;
  const dimension = dimensions[0] ? fieldLabel(dataset, dimensions[0]) : '';
  return dimension ? `${measure} by ${dimension}` : measure;
}

function filteredFields(dataset: ReportDatasetDefinition, query: string) {
  const term = query.trim().toLowerCase();
  const matches = (field: { field?: string; id?: string; label: string }) => {
    const key = field.field ?? field.id ?? '';
    return (
      term.length === 0 ||
      key.toLowerCase().includes(term) ||
      field.label.toLowerCase().includes(term)
    );
  };
  return {
    dimensions: visibleDimensions(dataset)
      .filter(matches)
      .map<CatalogField>((dimension) => ({
        id: dimension.field,
        label: dimension.label,
        type: dimension.type,
        format: dimension.format,
        kind: 'dimension',
      })),
    measures: visibleMeasures(dataset)
      .filter(matches)
      .map<CatalogField>((measure) => ({
        id: measure.id,
        label: measure.label,
        type: 'measure',
        format: measure.format,
        kind: 'measure',
      })),
  };
}

function visibleDimensions(dataset: ReportDatasetDefinition) {
  return dataset.dimensions.filter(
    (dimension) => !(dimension as { hidden?: boolean }).hidden
  );
}

function visibleMeasures(dataset: ReportDatasetDefinition) {
  return dataset.measures.filter(
    (measure) => !(measure as { hidden?: boolean }).hidden
  );
}

function isExplorableDataset(dataset: ReportDatasetDefinition) {
  return (dataset as { explorable?: boolean }).explorable !== false;
}

function buildPreviewBlock(
  dataset: ReportDatasetDefinition,
  state: ExploreState,
  columns: ReportDatasetQueryColumn[]
): ReportBlockDefinition {
  const measure = dataset.measures.find(
    (candidate) => candidate.id === state.measures[0]
  );
  const chartKind = isChartViz(state.vizType) ? state.vizType : 'bar';
  const x = state.dimensions[0] ?? columnKey(columns[0]) ?? '';
  const type =
    state.vizType === 'metric'
      ? 'metric'
      : state.vizType === 'table'
        ? 'table'
        : 'chart';
  return {
    id: 'explore_preview',
    type,
    title: state.blockTitle || 'Explore preview',
    source: { schema: dataset.source.schema },
    dataset: {
      id: dataset.id,
      dimensions: state.dimensions,
      measures: state.measures,
      orderBy: state.sort,
      datasetFilters: state.filters.map(({ field, op, value }) => ({
        field,
        op,
        value,
      })),
      limit: state.limit,
    },
    table: {
      columns: tableColumnsFromDataset(columns),
      defaultSort: state.sort,
      pagination: {
        defaultPageSize: state.page.size,
        allowedPageSizes: [25, 50, 100],
      },
    },
    chart: {
      kind: chartKind,
      x,
      series: state.measures.map((field) => ({
        field,
        label: measureLabel(dataset, field),
      })),
    },
    metric: {
      valueField: state.measures[0] ?? '',
      label: measure?.label,
      format: measure?.format,
    },
  };
}

function buildSavedBlock(
  definition: { blocks: ReportBlockDefinition[] },
  dataset: ReportDatasetDefinition,
  state: ExploreState
): ReportBlockDefinition | null {
  if (state.measures.length === 0) return null;
  const id = uniqueBlockId(
    definition.blocks,
    slugify(state.blockTitle || 'explore-block')
  );
  const block = buildPreviewBlock(dataset, state, [
    ...state.dimensions.map((field) => ({
      key: field,
      label: fieldLabel(dataset, field),
      type: dimensionType(dataset, field),
      format: dataset.dimensions.find((dimension) => dimension.field === field)
        ?.format,
    })),
    ...state.measures.map((field) => ({
      key: field,
      label: measureLabel(dataset, field),
      type: 'measure',
      format: dataset.measures.find((measure) => measure.id === field)?.format,
    })),
  ]);
  return {
    ...block,
    id,
    title: state.blockTitle || humanizeFieldName(id),
    source: { schema: '' },
  };
}

function replaceBlockInDefinition(
  definition: ReportDefinition,
  blockId: string,
  block: ReportBlockDefinition
): ReportDefinition {
  return {
    ...definition,
    blocks: definition.blocks.map((candidate) =>
      candidate.id === blockId ? block : candidate
    ),
  };
}

function appendBlockToDefinition(
  definition: ReportDefinition,
  block: ReportBlockDefinition
): ReportDefinition {
  const blocks = [...definition.blocks, block];
  if ((definition.layout?.length ?? 0) > 0) {
    return {
      ...definition,
      blocks,
      layout: [
        ...(definition.layout ?? []),
        {
          id: `${block.id}_node`,
          type: 'block',
          blockId: block.id,
        },
      ],
    };
  }

  return {
    ...definition,
    blocks,
    markdown: `${definition.markdown ?? ''}\n\n{{ block.${block.id} }}`,
  };
}

function buildReportPath(
  reportId: string,
  definition: ReportDefinition,
  searchParams: URLSearchParams
): string {
  const params = new URLSearchParams();
  for (const filter of definition.filters) {
    const value = searchParams.get(filter.id);
    if (value !== null) {
      params.set(filter.id, value);
    }
  }
  const query = params.toString();
  return `/reports/${reportId}${query ? `?${query}` : ''}`;
}

function datasetResultToBlockResult(
  block: ReportBlockDefinition,
  data?: ReportDatasetQueryResponse | null
) {
  const columns = data?.columns.map(columnKey).filter(Boolean) ?? [];
  const rows = data?.rows ?? [];
  const measureField = block.metric?.valueField ?? columns[columns.length - 1];
  const valueIndex = columns.indexOf(measureField);
  return {
    type: block.type,
    status: rows.length > 0 ? ('ready' as const) : ('empty' as const),
    title: block.title,
    data: {
      columns,
      rows,
      page: data?.page,
      value: valueIndex >= 0 ? rows[0]?.[valueIndex] : undefined,
      valueField: measureField,
      label: block.metric?.label,
      format: block.metric?.format,
    },
  };
}

function tableColumnsFromDataset(
  columns: ReportDatasetQueryColumn[]
): ReportTableColumn[] {
  return columns
    .map((column) => ({
      field: columnKey(column),
      label: column.label,
      format: column.format,
    }))
    .filter((column) => column.field.length > 0);
}

function isChartViz(value: ExploreVizType): value is ReportChartKind {
  return !['table', 'metric'].includes(value);
}

function columnKey(column?: ReportDatasetQueryColumn): string {
  return column?.field ?? column?.key ?? '';
}

function fieldLabel(dataset: ReportDatasetDefinition, field: string): string {
  return (
    dataset.dimensions.find((dimension) => dimension.field === field)?.label ??
    humanizeFieldName(field)
  );
}

function measureLabel(dataset: ReportDatasetDefinition, id: string): string {
  return (
    dataset.measures.find((measure) => measure.id === id)?.label ??
    humanizeFieldName(id)
  );
}

function outputFieldLabel(dataset: ReportDatasetDefinition, field: string) {
  return (
    dataset.dimensions.find((dimension) => dimension.field === field)?.label ??
    dataset.measures.find((measure) => measure.id === field)?.label ??
    humanizeFieldName(field)
  );
}

function dimensionType(
  dataset: ReportDatasetDefinition,
  field: string
): string {
  return (
    dataset.dimensions.find((dimension) => dimension.field === field)?.type ??
    'string'
  );
}

function defaultFilterOp(
  dataset: ReportDatasetDefinition | undefined,
  field: string
) {
  const type = dataset?.dimensions.find(
    (dimension) => dimension.field === field
  )?.type;
  if (
    type === 'date' ||
    type === 'datetime' ||
    type === 'number' ||
    type === 'decimal'
  ) {
    return 'gte';
  }
  return 'eq';
}

function filterOpsForField(
  dataset: ReportDatasetDefinition | undefined,
  field: string
) {
  const type = dataset?.dimensions.find(
    (dimension) => dimension.field === field
  )?.type;
  if (
    type === 'date' ||
    type === 'datetime' ||
    type === 'number' ||
    type === 'decimal'
  ) {
    return [
      { value: 'eq', label: 'Equals' },
      { value: 'gte', label: 'At least' },
      { value: 'lte', label: 'At most' },
      { value: 'gt', label: 'Greater than' },
      { value: 'lt', label: 'Less than' },
    ];
  }
  if (type === 'boolean') {
    return [{ value: 'eq', label: 'Equals' }];
  }
  return [
    { value: 'eq', label: 'Equals' },
    { value: 'contains', label: 'Contains' },
  ];
}

function parseFilterValue(
  dataset: ReportDatasetDefinition | undefined,
  field: string,
  value: string
) {
  const type = dataset?.dimensions.find(
    (dimension) => dimension.field === field
  )?.type;
  if (type === 'number' || type === 'decimal') {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : value;
  }
  if (type === 'boolean') {
    return value.toLowerCase() === 'true';
  }
  return value;
}

function formatFilterValue(value: unknown): string {
  if (Array.isArray(value)) return value.map(formatFilterValue).join(', ');
  if (value === null || value === undefined) return '';
  if (value instanceof Date) return value.toISOString();
  return String(value);
}

function uniqueBlockId(
  blocks: ReportBlockDefinition[],
  baseId: string
): string {
  const fallback = baseId || 'explore_block';
  const existing = new Set(blocks.map((block) => block.id));
  if (!existing.has(fallback)) return fallback;
  let index = 2;
  while (existing.has(`${fallback}_${index}`)) {
    index += 1;
  }
  return `${fallback}_${index}`;
}

function uniqueBlockTitle(
  blocks: ReportBlockDefinition[],
  title: string
): string {
  const existing = new Set(blocks.map((block) => block.title ?? block.id));
  if (!existing.has(title)) return title;
  let index = 2;
  while (existing.has(`${title} ${index}`)) {
    index += 1;
  }
  return `${title} ${index}`;
}

function clampNumber(
  value: string,
  min: number,
  max: number,
  fallback: number
): number {
  const parsed = Number(value);
  if (!Number.isFinite(parsed)) return fallback;
  return Math.min(max, Math.max(min, Math.round(parsed)));
}

function isEmptyFilterValue(value: unknown): boolean {
  if (value === null || value === undefined) return true;
  if (typeof value === 'string') return value.trim().length === 0;
  if (Array.isArray(value)) return value.length === 0;
  return false;
}
