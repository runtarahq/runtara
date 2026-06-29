// Report DSL types — Phase 2 of the reports refactor.
//
// Backend types are sourced from the generated OpenAPI client. To regenerate:
//   npm run generate-api-runtime-local       # against a running server
//   npm run generate-api-runtime-offline     # via `cargo run --bin dump_openapi`
//
// This file:
//   - Re-exports backend types under their canonical names.
//   - Tightens a handful of "optional on the wire, always-present at runtime"
//     fields via `Omit + &` so FE call sites don't need `??` defaults.
//   - Declares a handful of FE-internal helper types not on the wire format.
//
// Why we use `Omit + &` rather than mapped types: a mapped type like
// `MakeRequired<T, K>` produces a *new* type that TS treats as structurally
// distinct from `T`, breaking assignment of API responses to the FE alias.
// `Omit + &` produces a flat object type with the override applied;
// assignment from `T` still requires a boundary cast, but consumer
// signatures remain simple.

import type {
  Condition as GenCondition,
  ReportBlockDefinition as GenReportBlockDefinition,
  ReportBlockType,
  ReportDatasetDefinition as GenReportDatasetDefinition,
  ReportDefinition as GenReportDefinition,
} from '../../generated/RuntaraRuntimeApi';

// ---------------------------------------------------------------------------
// Backend types re-exported verbatim.
// ---------------------------------------------------------------------------
export type {
  // Core
  ReportStatus,
  ReportBlockType,
  ReportSummary,
  // Layout — Phase 9 collapse: only Block + Grid (recursive) remain.
  ReportLayoutNode,
  ReportBlockLayoutNode,
  ReportGridLayoutNode,
  ReportGridLayoutItem,
  // Views
  ReportViewDefinition,
  ReportViewBreadcrumb,
  ReportTitleFromBlock,
  // Filters
  ReportFilterDefinition,
  ReportFilterType,
  ReportFilterTarget,
  ReportFilterOption,
  ReportFilterOptionsRequest,
  ReportFilterOptionsResponse,
  ReportFilterOptionsPage,
  ReportFilterOptionsMetadata,
  ReportLookupOptionsRequest,
  ReportLookupOptionsResponse,
  ReportLookupBlockMetadata,
  // Blocks (definition is tightened below)
  ReportBlockStatus,
  ReportBlockRenderResult,
  ReportBlockError,
  ReportBlockDataRequest,
  ReportBlockOnlyDataRequest,
  ReportBlockDatasetQuery,
  // Block configs
  ReportMarkdownConfig,
  ReportActionsConfig,
  ReportActionSubmitConfig,
  ReportTableConfig,
  ReportTableActionConfig,
  ReportTableColumn,
  ReportTableColumnType,
  ReportTableColumnSource,
  ReportTableColumnJoin,
  ReportTableInteractionButtonConfig,
  ReportChartConfig,
  ReportChartKind,
  ReportChartSeries,
  ReportMetricConfig,
  ReportCardConfig,
  ReportCardGroup,
  ReportCardField,
  ReportCardFieldKind,
  ReportSubtableConfig,
  ReportSubtableColumn,
  // Sources
  ReportSource,
  ReportSourceJoin,
  ReportSourceKind,
  ReportSourceMode,
  ReportJoinKind,
  ReportWorkflowRuntimeEntity,
  ReportAggregateSpec,
  ReportAggregateFn,
  ReportOrderBy,
  ReportPaginationConfig,
  // Datasets (definition is tightened below)
  ReportDatasetSource,
  ReportDatasetDimension,
  ReportDatasetMeasure,
  ReportDatasetFieldType,
  ReportDatasetValueFormat,
  ReportDatasetQueryRequest,
  ReportDatasetQueryResponse,
  ReportDatasetQueryColumn,
  ReportDatasetQueryMetadata,
  ReportDatasetQueryPage,
  ReportDatasetFilter,
  // Workflow actions
  ReportWorkflowActionConfig,
  ReportWorkflowActionContext,
  ReportWorkflowActionContextMode,
  SubmitReportWorkflowActionRequest,
  // Interactions (definition is tightened below)
  ReportInteractionTrigger,
  ReportInteractionAction,
  // Editors
  ReportEditorConfig,
  ReportEditorOption,
  ReportEditorKind,
  ReportLookupConfig,
  // Search
  ReportTableSearchRequest,
  // Pagination
  ReportPageRequest,
  // Render
  ReportRenderRequest,
  ReportRenderResponse,
  ReportRenderMetadata,
  ReportPreviewRequest,
  // Mutations
  CreateReportRequest,
  UpdateReportRequest,
  DeleteReportResponse,
  // Canonical /edit endpoint (Phase 6/8) — batch ReportEditOps applied
  // atomically by `runtara_report_dsl::edit_ops::apply_edit_ops`.
  EditReportRequest,
  EditReportResponse,
  ReportEditOp,
  BlockPosition,
  LayoutTarget,
  // Lists
  ListReportsResponse,
  GetReportResponse,
  // Validation
  ValidateReportRequest,
  ValidateReportResponse,
  ReportValidationIssue,
  // Generic
  Condition,
} from '../../generated/RuntaraRuntimeApi';

// ---------------------------------------------------------------------------
// Tightened types — these fields are optional on the wire (Rust `Option<Vec>`
// with `#[serde(default)]`) but always populated at runtime. The FE uses them
// without null checks throughout. Boundary helpers in `queries/index.ts`
// cast API responses up to this stricter shape.
// ---------------------------------------------------------------------------

export type ReportBlockDefinition = Omit<GenReportBlockDefinition, 'source'> & {
  source: import('../../generated/RuntaraRuntimeApi').ReportSource;
};

export type ReportDatasetDefinition = Omit<
  GenReportDatasetDefinition,
  'dimensions' | 'measures'
> & {
  dimensions: import('../../generated/RuntaraRuntimeApi').ReportDatasetDimension[];
  measures: import('../../generated/RuntaraRuntimeApi').ReportDatasetMeasure[];
};

// `ReportDefinition` is the parent of the others — its array fields use the
// FE-tightened element types so consumers walking the tree never re-fall
// back to generated optional fields. `layout` is the mandatory single root
// grid (Phase 10) — the generated type marks it optional because the wire
// `default_root_grid` fallback covers missing payloads, but the FE always
// sees a populated value (server-side migration guarantees this) so we
// tighten to non-optional here.
export type ReportDefinition = Omit<
  GenReportDefinition,
  'blocks' | 'filters' | 'datasets' | 'layout'
> & {
  blocks: ReportBlockDefinition[];
  filters: import('../../generated/RuntaraRuntimeApi').ReportFilterDefinition[];
  datasets?: ReportDatasetDefinition[];
  layout: import('../../generated/RuntaraRuntimeApi').ReportGridLayoutNode;
};

// `ReportDto` carries the definition; we surface the FE-tightened version
// so consumers walking `report.definition.blocks` get the FE element type.
export type ReportDto = Omit<
  import('../../generated/RuntaraRuntimeApi').ReportDto,
  'definition'
> & {
  definition: ReportDefinition;
};

// ---------------------------------------------------------------------------
// FE-only types — not present in the wire format.
// ---------------------------------------------------------------------------

/** Filter/source condition (legacy shape — `{op, arguments: [field, value]}`). */
export type ReportCondition = GenCondition;

/**
 * Row visibility/disability condition. Canonical `ConditionExpression`
 * shape — the same evaluator used for workflow steps.
 */
export type ReportRowCondition =
  import('../../generated/RuntaraRuntimeApi').ConditionExpression;

/** Visibility-condition shape used on layout nodes (`showWhen`). FE-internal. */
export interface ReportVisibilityCondition {
  filter: string;
  exists?: boolean;
  equals?: unknown;
  notEquals?: unknown;
}

/** FE filter options config used by the filter editor; mirrors the wire shape
 * with a few FE-only refinements. */
export interface ReportFilterOptionsConfig {
  source?: 'static' | 'object_model';
  values?: Array<{ label: string; value: unknown; count?: number }>;
  schema?: string;
  field?: string;
  valueField?: string;
  labelField?: string;
  connectionId?: string;
  search?: boolean;
  dependsOn?: string[];
  filterMappings?: Array<{
    filterId: string;
    field: string;
    op?: string;
  }>;
  condition?: GenCondition;
}

/** Block render state as observed by the FE renderer.
 *
 * Strict supertype of the generated `ReportBlockRenderResult`:
 *   - same shape on the wire
 *   - FE additionally surfaces a `'loading'` state during fetches
 *   - `status` stays optional so the generated form (`status?:
 *     ReportBlockStatus`) assigns cleanly through this alias.
 */
export interface ReportBlockResult {
  type: ReportBlockType;
  status?: 'ready' | 'loading' | 'empty' | 'error';
  title?: string | null;
  data?: unknown;
  error?: import('../../generated/RuntaraRuntimeApi').ReportBlockError | null;
}

/** Action used by the workflow polling hooks. */
export interface ReportWorkflowAction {
  id: string;
  actionId: string;
  actionKind: string;
  targetKind: string;
  targetId: string;
  workflowId: string;
  instanceId: string;
  signalId: string;
  actionKey?: string | null;
  label: string;
  message: string;
  inputSchema?: Record<string, unknown> | null;
  schemaFormat: string;
  status: string;
  requestedAt?: string | null;
  correlation?: Record<string, unknown>;
  context?: Record<string, unknown>;
  runtime?: Record<string, unknown>;
}

/** Per-instance status surfaced to the FE while polling. */
export interface ReportWorkflowInstanceStatus {
  id: string;
  status: string;
}

/** Run-workflow request sent by FE workflow-button handlers. */
export interface RunReportWorkflowRequest {
  workflowId: string;
  version?: number;
  context: unknown;
}

/** Run-workflow response received by FE workflow-button handlers. */
export interface RunReportWorkflowResponse {
  instanceId: string;
  status: string;
}

/** Options FE passes to its set-filter / navigate-view interaction handler. */
export interface ReportInteractionOptions {
  replace?: boolean;
  viewId?: string | null;
  clearFilters?: string[];
}

/** Dataset filter request shape — historical FE alias of `ReportDatasetFilter`. */
export type ReportDatasetFilterRequest =
  import('../../generated/RuntaraRuntimeApi').ReportDatasetFilter;
