import { useEffect, useMemo, useState } from 'react';
import { Link, useNavigate, useParams, useSearchParams } from 'react-router';
import { Compass, Edit, Eye, Printer, RefreshCw, Save } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { TileList, TilesPage } from '@/shared/components/tiles-page';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { useObjectSchemaDtos } from '@/features/objects/hooks/useObjectSchemas';
import {
  useCreateReport,
  useReport,
  useReportPreview,
  useReportRender,
  useUpdateReport,
  useValidateReport,
} from '../hooks/useReports';
import { ReportDeleteButton } from '../components/ReportDeleteButton';
import { ReportFilterBar } from '../components/ReportFilterBar';
import { ReportBuilderWizard } from '../components/wizard/ReportBuilderWizard';
import { ReportRenderer } from '../components/ReportRenderer';
import {
  ReportBlockResult,
  ReportDefinition,
  ReportInteractionOptions,
} from '../types';
import {
  decodeFilterValue,
  encodeFilterValue,
  getFilterDefaultValue,
  getDefaultReportViewId,
  slugify,
} from '../utils';

const EMPTY_DEFINITION: ReportDefinition = {
  definitionVersion: 1,
  layout: [],
  filters: [],
  blocks: [],
};

/** Unified report page. Same DOM in view and edit modes — toggling `?edit=1`
 *  swaps the header chrome and shows/hides editing affordances inside the
 *  layout, but the report itself (grids, blocks, real-data previews) renders
 *  identically in both modes. */
export function ReportPage() {
  const { reportId } = useParams();
  const isExisting = Boolean(reportId);
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const editing = searchParams.get('edit') === '1' || !isExisting;

  const { data: existingReport, isFetching } = useReport(reportId);
  const { data: schemas = [] } = useObjectSchemaDtos();
  const createReport = useCreateReport();
  const updateReport = useUpdateReport();
  const validateReport = useValidateReport();
  const activeViewId = searchParams.get('view');

  usePageTitle(existingReport?.name ?? (isExisting ? 'Report' : 'New report'));

  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [definition, setDefinition] =
    useState<ReportDefinition>(EMPTY_DEFINITION);
  const [saveError, setSaveError] = useState<string | null>(null);

  useEffect(() => {
    if (!existingReport) return;
    setName(existingReport.name);
    setDescription(existingReport.description ?? '');
    setDefinition(existingReport.definition);
  }, [existingReport]);

  // Filter values come from URL params in view mode; in edit mode we still
  // honor defaults so the preview reflects what viewers will see.
  const filterValues = useMemo(() => {
    return Object.fromEntries(
      (definition.filters ?? []).map((filter) => [
        filter.id,
        decodeFilterValue(filter, searchParams.get(filter.id)),
      ])
    );
  }, [definition.filters, searchParams]);

  // Debounce the definition so the preview API isn't pummelled while typing.
  const [debouncedDefinition, setDebouncedDefinition] =
    useState<ReportDefinition>(EMPTY_DEFINITION);
  useEffect(() => {
    const handle = setTimeout(() => setDebouncedDefinition(definition), 400);
    return () => clearTimeout(handle);
  }, [definition]);

  const canPreview = useMemo(
    () =>
      editing &&
      debouncedDefinition.blocks.some(
        (block) =>
          block.type === 'markdown' ||
          (block.source?.schema && block.source.schema.length > 0)
      ),
    [debouncedDefinition, editing]
  );

  const previewRequest = useMemo(
    () =>
      canPreview
        ? { filters: filterValues, definition: debouncedDefinition }
        : undefined,
    [canPreview, debouncedDefinition, filterValues]
  );

  const previewQuery = useReportPreview(previewRequest, canPreview);
  const blockResults: Record<string, ReportBlockResult> = useMemo(
    () => previewQuery.data?.blocks ?? {},
    [previewQuery.data]
  );
  const renderRequest = useMemo(
    () =>
      !editing && isExisting
        ? {
            filters: filterValues,
            timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
          }
        : undefined,
    [editing, filterValues, isExisting]
  );
  const renderQuery = useReportRender(
    reportId,
    renderRequest,
    Boolean(renderRequest)
  );

  const canSave =
    name.trim().length > 0 &&
    !createReport.isPending &&
    !updateReport.isPending &&
    !validateReport.isPending;

  const handleFilterChange = (filterId: string, value: unknown) => {
    setSearchParams(
      (current) => {
        const next = new URLSearchParams(current);
        const filter = definition.filters.find((f) => f.id === filterId);
        const defaultValue = filter ? getFilterDefaultValue(filter) : undefined;
        if (
          isEmptyFilterValue(value) ||
          isSameFilterValue(value, defaultValue)
        ) {
          next.delete(filterId);
        } else {
          next.set(filterId, encodeFilterValue(value));
        }
        return next;
      },
      { replace: true }
    );
  };

  const applyFilterUpdates = (
    updates: Record<string, unknown>,
    options?: ReportInteractionOptions
  ) => {
    setSearchParams(
      (current) => {
        const next = new URLSearchParams(current);
        for (const filterId of options?.clearFilters ?? []) {
          next.delete(filterId);
        }
        for (const [filterId, value] of Object.entries(updates)) {
          const filter = definition.filters.find((f) => f.id === filterId);
          const defaultValue = filter
            ? getFilterDefaultValue(filter)
            : undefined;
          if (
            isEmptyFilterValue(value) ||
            isSameFilterValue(value, defaultValue)
          ) {
            next.delete(filterId);
          } else {
            next.set(filterId, encodeFilterValue(value));
          }
        }
        if (options?.viewId !== undefined) {
          if (options.viewId) next.set('view', options.viewId);
          else next.delete('view');
        }
        return next;
      },
      { replace: options?.replace ?? true }
    );
  };

  const handleNavigateView = (
    viewId: string | null,
    options?: Omit<ReportInteractionOptions, 'viewId'>
  ) => {
    applyFilterUpdates({}, { ...options, viewId });
  };

  const handleSave = async () => {
    setSaveError(null);
    const validation = await validateReport.mutateAsync({ definition });
    if (!validation.valid) {
      setSaveError(validation.errors[0]?.message ?? 'Report is invalid.');
      return;
    }
    const trimmedName = name.trim();
    const payload = {
      name: trimmedName,
      slug: slugify(trimmedName),
      description: description.trim() || null,
      tags: [],
      status: 'published' as const,
      definition,
    };
    if (isExisting && reportId) {
      const report = await updateReport.mutateAsync({
        id: reportId,
        data: payload,
      });
      navigate(`/reports/${report.id}?edit=1`);
    } else {
      const report = await createReport.mutateAsync(payload);
      navigate(`/reports/${report.id}?edit=1`);
    }
  };

  const handlePrint = () => {
    window.requestAnimationFrame(() => window.print());
  };

  // Mount the wizard only after the loaded definition has flowed into local
  // state. Without this the wizard initializes from EMPTY_DEFINITION (because
  // its `useMemo([])` runs on the first render, before the `setDefinition`
  // useEffect copies `existingReport.definition`) and existing reports render
  // as a blank canvas.
  const awaitingDefinition =
    isExisting &&
    (isFetching || !existingReport || definition === EMPTY_DEFINITION);

  if (awaitingDefinition) {
    return (
      <TilesPage kicker="Reports" title="Loading report">
        <TileList>
          <div className="h-96 animate-pulse rounded-xl bg-muted/30" />
        </TileList>
      </TilesPage>
    );
  }

  const titleNode = editing ? (
    <input
      value={name}
      placeholder="Untitled report"
      onChange={(event) => setName(event.target.value)}
      className="w-full bg-transparent text-xl font-semibold placeholder:text-muted-foreground focus:outline-none"
      style={{ border: 'none', outline: 'none', boxShadow: 'none', padding: 0 }}
    />
  ) : (
    <span>{name || 'Untitled report'}</span>
  );

  const toolbar = (
    <div className="flex flex-col gap-2">
      {editing ? (
        <input
          value={description}
          placeholder="Optional description shown in the reports list…"
          onChange={(event) => setDescription(event.target.value)}
          className="w-full bg-transparent text-sm text-muted-foreground placeholder:text-muted-foreground focus:outline-none"
          style={{
            border: 'none',
            outline: 'none',
            boxShadow: 'none',
            padding: 0,
          }}
        />
      ) : description ? (
        <p className="text-sm text-muted-foreground">{description}</p>
      ) : null}
      {definition.filters.length > 0 && reportId ? (
        <ReportFilterBar
          reportId={reportId}
          definition={definition}
          values={filterValues}
          onChange={handleFilterChange}
        />
      ) : null}
    </div>
  );

  const exploreSearch = searchParams.toString();
  const explorePath =
    isExisting && reportId
      ? `/reports/${reportId}/explore${exploreSearch ? `?${exploreSearch}` : ''}`
      : null;

  const actions = (
    <div className="flex w-full flex-col gap-2 sm:w-auto sm:flex-row">
      {editing ? (
        isExisting && reportId ? (
          <Link to={`/reports/${reportId}`} className="w-full sm:w-auto">
            <Button
              variant="outline"
              className="h-11 w-full rounded-full sm:px-5"
            >
              <Eye className="mr-2 h-4 w-4" />
              View
            </Button>
          </Link>
        ) : (
          <Link to="/reports" className="w-full sm:w-auto">
            <Button
              variant="outline"
              className="h-11 w-full rounded-full sm:px-5"
            >
              Cancel
            </Button>
          </Link>
        )
      ) : (
        <>
          {explorePath ? (
            <Link to={explorePath} className="w-full sm:w-auto">
              <Button
                variant="outline"
                className="h-11 w-full rounded-full sm:px-5"
              >
                <Compass className="mr-2 h-4 w-4" />
                Explore
              </Button>
            </Link>
          ) : null}
          <Button
            variant="outline"
            className="h-11 rounded-full sm:px-5"
            disabled={renderQuery.isFetching}
            onClick={handlePrint}
          >
            <Printer className="mr-2 h-4 w-4" />
            Print
          </Button>
          <Button
            variant="outline"
            className="h-11 rounded-full sm:px-5"
            disabled={renderQuery.isFetching}
            onClick={() => renderQuery.refetch()}
          >
            <RefreshCw className="mr-2 h-4 w-4" />
            Refresh
          </Button>
          <Link to={`/reports/${reportId}?edit=1`} className="w-full sm:w-auto">
            <Button
              variant="outline"
              className="h-11 w-full rounded-full sm:px-5"
            >
              <Edit className="mr-2 h-4 w-4" />
              Edit
            </Button>
          </Link>
        </>
      )}
      {editing && isExisting && reportId && existingReport ? (
        <ReportDeleteButton
          reportId={reportId}
          reportName={existingReport.name}
          className="h-11 rounded-full sm:px-5"
        />
      ) : null}
      {editing ? (
        <Button
          className="h-11 rounded-full sm:px-5"
          disabled={!canSave}
          onClick={handleSave}
        >
          <Save className="mr-2 h-4 w-4" />
          Save
        </Button>
      ) : null}
    </div>
  );

  return (
    <TilesPage
      kicker="Reports"
      title={titleNode}
      toolbar={toolbar}
      action={actions}
      className={!editing ? 'report-print-root' : undefined}
      contentClassName={!editing ? 'report-print-content pb-16' : undefined}
    >
      {editing ? (
        <ReportBuilderWizard
          // Force a fresh wizard state when the loaded report changes; the
          // wizard derives its initial state via useMemo([]) so it would
          // otherwise keep the previously-loaded report's blocks.
          key={reportId ?? 'new'}
          definition={definition}
          schemas={schemas}
          blockResults={blockResults}
          editing={editing}
          onChange={(nextDefinition) => {
            setDefinition(nextDefinition);
            setSaveError(null);
            validateReport.reset();
          }}
        />
      ) : reportId ? (
        <ReportRenderer
          reportId={reportId}
          definition={definition}
          renderResponse={renderQuery.data}
          filters={filterValues}
          activeViewId={activeViewId ?? getDefaultReportViewId(definition)}
          onFilterChange={handleFilterChange}
          onFiltersChange={applyFilterUpdates}
          onNavigateView={handleNavigateView}
          onRefresh={() => renderQuery.refetch()}
        />
      ) : null}
      {saveError ? (
        <p className="mt-3 text-sm text-destructive">{saveError}</p>
      ) : null}
    </TilesPage>
  );
}

function isEmptyFilterValue(value: unknown): boolean {
  if (value === null || value === undefined) return true;
  if (typeof value === 'string') return value.trim().length === 0;
  if (Array.isArray(value)) return value.length === 0;
  return false;
}

function isSameFilterValue(left: unknown, right: unknown): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}
