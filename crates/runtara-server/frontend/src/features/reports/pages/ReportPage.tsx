import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { Link, useNavigate, useParams, useSearchParams } from 'react-router';
import {
  AlertTriangle,
  Compass,
  Edit,
  Eye,
  Printer,
  RefreshCw,
  Save,
} from 'lucide-react';
import {
  Alert,
  AlertDescription,
  AlertTitle,
} from '@/shared/components/ui/alert';
import { Button } from '@/shared/components/ui/button';
import { Can } from '@/shared/components/Can';
import { TilesPage } from '@/shared/components/tiles-page';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { useObjectSchemaDtosByConnectionIds } from '@/features/objects/hooks/useObjectSchemas';
import { ObjectModelConnectionSelector } from '@/features/objects/components/ObjectModelConnectionSelector';
import { useObjectModelConnectionSelection } from '@/features/objects/hooks/useObjectModelConnectionSelection';
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
import { ReportRenderer } from '../components/ReportRenderer';
import {
  ReportBlockResult,
  ReportDefinition,
  ReportInteractionOptions,
} from '../types';
import {
  decodeFilterValue,
  encodeFilterValue,
  getCanonicalReportViewTarget,
  getFilterDefaultValue,
  getDefaultReportViewId,
  getDefaultReportViewTarget,
  getReportLayoutBlockIds,
  slugify,
} from '../utils';

// Wizard v2 — operates on ReportDefinition directly, no WizardState
// intermediate model. Default authoring surface as of Phase 7 cutover.
// Lazy-loaded so view-only sessions don't pay the parse cost.
const ReportBuilderWizardV2 = lazy(() =>
  import('../components/wizard-v2/ReportBuilderWizardV2').then((m) => ({
    default: m.ReportBuilderWizardV2,
  }))
);

const EMPTY_DEFINITION: ReportDefinition = {
  definitionVersion: 1,
  layout: { id: 'root', columns: 1, rows: 1, items: [] },
  filters: [],
  blocks: [],
};

const waitForReportState = (milliseconds: number) =>
  new Promise<void>((resolve) => window.setTimeout(resolve, milliseconds));

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
  const { selectedConnectionId, connections: objectModelConnections } =
    useObjectModelConnectionSelection();
  const objectModelSchemaConnectionIds = useMemo(
    () =>
      Array.from(
        new Set(
          [
            selectedConnectionId,
            ...objectModelConnections.map((connection) => connection.id),
          ].filter((id): id is string => Boolean(id))
        )
      ),
    [objectModelConnections, selectedConnectionId]
  );
  const { schemasByConnectionId } = useObjectSchemaDtosByConnectionIds(
    objectModelSchemaConnectionIds
  );

  const {
    data: existingReport,
    isPending: reportLoading,
    isPlaceholderData: reportShowingPlaceholder,
  } = useReport(reportId);
  const schemas = selectedConnectionId
    ? (schemasByConnectionId[selectedConnectionId] ?? [])
    : [];
  const createReport = useCreateReport();
  const updateReport = useUpdateReport();
  const validateReport = useValidateReport();
  const requestedViewId = searchParams.get('view');

  usePageTitle(existingReport?.name ?? (isExisting ? 'Report' : 'New report'));

  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [definition, setDefinition] =
    useState<ReportDefinition>(EMPTY_DEFINITION);
  const [saveError, setSaveError] = useState<string | null>(null);
  // The server flags reports whose stored JSON no longer fits the
  // current `ReportDefinition` shape. We hold that flag in local state
  // (a) so the banner stays sticky after the user opts into
  // re-authoring; (b) so the wizard never starts from the empty stub
  // until the operator explicitly chooses to.
  const [needsReAuthoring, setNeedsReAuthoring] = useState<string | null>(null);
  const [reAuthoringAccepted, setReAuthoringAccepted] = useState(false);

  useEffect(() => {
    if (!existingReport) return;
    setName(existingReport.name);
    setDescription(existingReport.description ?? '');
    setDefinition(existingReport.definition);
    setNeedsReAuthoring(existingReport.needsReAuthoring ?? null);
    setReAuthoringAccepted(false);
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

  const renderRequest = useMemo(
    () =>
      !editing && isExisting
        ? {
            filters: filterValues,
            viewId:
              requestedViewId ??
              getDefaultReportViewTarget(definition) ??
              undefined,
            timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
          }
        : undefined,
    [definition, editing, filterValues, isExisting, requestedViewId]
  );
  const renderQuery = useReportRender(
    reportId,
    renderRequest,
    Boolean(renderRequest)
  );
  const responseMatchesRequest =
    (renderQuery.data?.navigation?.requestedViewId ?? null) ===
    (renderRequest?.viewId ?? null);
  const resolvedActiveViewId =
    (responseMatchesRequest
      ? renderQuery.data?.navigation?.activeViewId
      : undefined) ??
    requestedViewId ??
    getDefaultReportViewId(definition);
  const visibleBlockIds = useMemo(
    () =>
      editing
        ? null
        : getReportLayoutBlockIds(definition, resolvedActiveViewId),
    [definition, editing, resolvedActiveViewId]
  );
  const previousStageRef = useRef<{
    reportId?: string;
    groupId?: string;
    currentViewId: string | null;
  }>({ currentViewId: null });

  useEffect(() => {
    const navigation = renderQuery.data?.navigation;
    const submittedViewId = renderRequest?.viewId ?? null;
    if (
      editing ||
      !navigation ||
      (navigation.requestedViewId ?? null) !== submittedViewId
    ) {
      return;
    }
    const groupId = navigation.group?.id;
    const previous = previousStageRef.current;
    const previousCurrentViewId =
      previous.reportId === reportId && previous.groupId === groupId
        ? previous.currentViewId
        : null;
    const canonicalViewId = getCanonicalReportViewTarget(
      definition,
      submittedViewId,
      navigation,
      previousCurrentViewId
    );
    previousStageRef.current = {
      reportId,
      groupId,
      currentViewId: navigation.group?.currentViewId ?? null,
    };
    if (!canonicalViewId || requestedViewId === canonicalViewId) return;

    setSearchParams(
      (current) => {
        const next = new URLSearchParams(current);
        next.set('view', canonicalViewId);
        return next;
      },
      { replace: true }
    );
  }, [
    definition,
    editing,
    renderQuery.data?.navigation,
    renderRequest?.viewId,
    reportId,
    requestedViewId,
    setSearchParams,
  ]);

  const handleReportActionRefresh = useCallback(async () => {
    const startingNavigation = renderQuery.data?.navigation;
    const startingGroup = startingNavigation?.group;
    const group = (definition.viewGroups ?? []).find(
      (candidate) => candidate.id === startingGroup?.id
    );
    const startingCurrentViewId = startingGroup?.currentViewId;
    const shouldPoll = Boolean(
      group?.mode === 'stages' &&
        group.followCurrentOnAdvance &&
        startingCurrentViewId &&
        startingNavigation?.activeViewId === startingCurrentViewId
    );
    const delays = shouldPoll ? [0, 400, 800, 1600, 2400] : [0];
    let result: Awaited<ReturnType<typeof renderQuery.refetch>> | undefined;

    for (const delay of delays) {
      if (delay > 0) await waitForReportState(delay);
      result = await renderQuery.refetch();
      const nextCurrentViewId = result.data?.navigation?.group?.currentViewId;
      if (!shouldPoll) break;
      if (nextCurrentViewId && nextCurrentViewId !== startingCurrentViewId) {
        setSearchParams(
          (current) => {
            const next = new URLSearchParams(current);
            next.set('view', nextCurrentViewId);
            return next;
          },
          { replace: true }
        );
        break;
      }
    }
    return result;
  }, [definition.viewGroups, renderQuery, setSearchParams]);

  // Phase 9: in-place block preview for the wizard. Debounced from the
  // live definition so live edits don't pummel the preview API.
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
        ? {
            filters: filterValues,
            definition: debouncedDefinition,
          }
        : undefined,
    [canPreview, debouncedDefinition, filterValues]
  );
  const previewQuery = useReportPreview(previewRequest, canPreview);
  const blockResults: Partial<Record<string, ReportBlockResult>> = useMemo(
    () => previewQuery.data?.blocks ?? {},
    [previewQuery.data]
  );

  const canSave =
    name.trim().length > 0 &&
    !createReport.isPending &&
    !updateReport.isPending &&
    !validateReport.isPending &&
    (!needsReAuthoring || reAuthoringAccepted);

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
    const validation = await validateReport.mutateAsync({
      definition,
    });
    if (!validation.valid) {
      setSaveError(validation.errors?.[0]?.message ?? 'Report is invalid.');
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
    (reportLoading ||
      reportShowingPlaceholder ||
      !existingReport ||
      definition === EMPTY_DEFINITION);

  if (awaitingDefinition) {
    return (
      <TilesPage kicker="Reports" title="Loading report…">
        <ReportSkeleton />
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
          visibleBlockIds={visibleBlockIds}
        />
      ) : null}
      {editing ? (
        <div className="mt-2">
          <ObjectModelConnectionSelector />
        </div>
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
            <Button variant="outline" className="w-full sm:px-4">
              <Eye className="mr-2 h-4 w-4" />
              View
            </Button>
          </Link>
        ) : (
          <Link to="/reports" className="w-full sm:w-auto">
            <Button variant="outline" className="w-full sm:px-4">
              Cancel
            </Button>
          </Link>
        )
      ) : (
        <>
          {explorePath ? (
            <Link to={explorePath} className="w-full sm:w-auto">
              <Button variant="outline" className="w-full sm:px-4">
                <Compass className="mr-2 h-4 w-4" />
                Explore
              </Button>
            </Link>
          ) : null}
          <Button
            variant="outline"
            className="sm:px-4"
            disabled={renderQuery.isFetching}
            onClick={handlePrint}
          >
            <Printer className="mr-2 h-4 w-4" />
            Print
          </Button>
          <Button
            variant="outline"
            className="sm:px-4"
            disabled={renderQuery.isFetching}
            onClick={() => void handleReportActionRefresh()}
          >
            <RefreshCw className="mr-2 h-4 w-4" />
            Refresh
          </Button>
          {isExisting && reportId && existingReport ? (
            <Can permission="report:delete">
              <ReportDeleteButton
                reportId={reportId}
                reportName={existingReport.name}
                className="sm:px-4"
              />
            </Can>
          ) : null}
          <Can permission="report:update">
            <Link
              to={`/reports/${reportId}?edit=1`}
              className="w-full sm:w-auto"
            >
              <Button variant="outline" className="w-full sm:px-4">
                <Edit className="mr-2 h-4 w-4" />
                Edit
              </Button>
            </Link>
          </Can>
        </>
      )}
      {editing && isExisting && reportId && existingReport ? (
        <Can permission="report:delete">
          <ReportDeleteButton
            reportId={reportId}
            reportName={existingReport.name}
            className="sm:px-4"
          />
        </Can>
      ) : null}
      {editing ? (
        <Can permission="report:update">
          <Button className="sm:px-4" disabled={!canSave} onClick={handleSave}>
            <Save className="mr-2 h-4 w-4" />
            Save
          </Button>
        </Can>
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
      {needsReAuthoring ? (
        <Alert className="mb-4 border-amber-400/60 bg-amber-50 dark:bg-amber-950/30">
          <AlertTriangle className="h-4 w-4 text-amber-600" />
          <AlertTitle>This report needs re-authoring</AlertTitle>
          <AlertDescription>
            <p className="mb-2 text-sm">
              The stored definition no longer fits the current schema and has
              been replaced with an empty stub. Saving now will
              <strong> overwrite the stored JSON</strong> — the previous content
              is preserved on the server until you do.
            </p>
            <p className="mb-3 text-xs text-muted-foreground">
              Reason: <code>{needsReAuthoring}</code>
            </p>
            {editing ? (
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => setReAuthoringAccepted(true)}
                disabled={reAuthoringAccepted}
                data-testid="confirm-reauthor"
              >
                {reAuthoringAccepted
                  ? 'Re-authoring confirmed — Save is enabled'
                  : 'Re-author from scratch'}
              </Button>
            ) : null}
          </AlertDescription>
        </Alert>
      ) : null}
      {editing ? (
        needsReAuthoring && !reAuthoringAccepted ? (
          <div
            className="rounded-md border border-dashed bg-muted/20 p-6 text-sm text-muted-foreground"
            data-testid="reauthoring-editor-locked"
          >
            Editing is disabled until you click "Re-author from scratch" above.
            This protects the stored definition from being overwritten by the
            empty stub the loader fell back to.
          </div>
        ) : (
          <Suspense
            fallback={
              <div className="h-96 animate-pulse rounded-lg bg-muted/30" />
            }
          >
            <ReportBuilderWizardV2
              key={reportId ?? 'new'}
              definition={definition}
              schemas={schemas}
              editing={editing}
              blockResults={blockResults}
              filters={filterValues}
              reportId={reportId}
              onChange={(nextDefinition) => {
                setDefinition(nextDefinition);
                setSaveError(null);
                validateReport.reset();
              }}
            />
          </Suspense>
        )
      ) : needsReAuthoring ? (
        <div
          className="rounded-md border border-dashed bg-muted/20 p-6 text-sm text-muted-foreground"
          data-testid="reauthoring-viewer-locked"
        >
          The report can't be rendered until it's re-authored. Open it in edit
          mode to start.
        </div>
      ) : reportId ? (
        <ReportRenderer
          reportId={reportId}
          definition={definition}
          renderResponse={renderQuery.data}
          filters={filterValues}
          activeViewId={resolvedActiveViewId}
          onFilterChange={handleFilterChange}
          onFiltersChange={applyFilterUpdates}
          onNavigateView={handleNavigateView}
          onRefresh={handleReportActionRefresh}
        />
      ) : null}
      {saveError ? (
        <p className="mt-3 text-sm text-destructive">{saveError}</p>
      ) : null}
    </TilesPage>
  );
}

/** Report-shaped loading placeholder shown while the definition loads, so
 *  opening a report reads as "a report is loading" rather than a blank slab. */
function ReportSkeleton() {
  return (
    <div className="space-y-6">
      <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
        {Array.from({ length: 4 }).map((_, index) => (
          <div
            key={index}
            className="h-24 animate-pulse rounded-lg border bg-muted/30"
          />
        ))}
      </div>
      <div className="h-72 animate-pulse rounded-lg border bg-muted/30" />
      <div className="overflow-hidden rounded-lg border">
        <div className="h-9 animate-pulse bg-muted/40" />
        <div className="divide-y">
          {Array.from({ length: 6 }).map((_, index) => (
            <div key={index} className="flex items-center gap-4 px-3 py-2.5">
              <div className="h-3 w-1/4 animate-pulse rounded bg-muted/40" />
              <div className="h-3 w-1/3 animate-pulse rounded bg-muted/40" />
              <div className="ml-auto h-3 w-16 animate-pulse rounded bg-muted/40" />
            </div>
          ))}
        </div>
      </div>
    </div>
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
