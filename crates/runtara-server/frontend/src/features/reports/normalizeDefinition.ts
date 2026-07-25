// Authoring-side normalization applied when the wizard saves a report.
//
// The wizard is deliberately lossless: it edits `ReportDefinition` in place
// and never mutates the loaded definition on mount (see
// `wizard-v2/__tests__/losslessRoundTrip.test.tsx`). The one thing it *does*
// owe the stored definition is the DSL's own read-time inference, so the
// authoring surface and the renderer never disagree about what a column is.

import type {
  ReportBlockDefinition,
  ReportDefinition,
  ReportTableColumn,
  ReportTableColumnType,
} from './types';

/**
 * Resolve a table column's effective type.
 *
 * `ReportTableColumn.type` is optional in the DSL: a column that carries a
 * `workflowAction` is a workflow button, and one that carries
 * `interactionButtons` is a button group, whether or not `type` is set.
 * Both readers already apply this inference —
 * `ReportTableColumn::is_workflow_button` /
 * `is_interaction_buttons` in `crates/runtara-report-dsl/src/types.rs`, and
 * `isWorkflowButtonColumn` / `isInteractionButtonsColumn` in
 * `components/blocks/tableLayout.ts`. The wizard must agree, otherwise it
 * shows an action column as a plain "Value" and hides its action sub-editor.
 */
export function inferReportTableColumnType(
  column: ReportTableColumn
): ReportTableColumnType {
  if (column.type) return column.type;
  if (column.workflowAction) return 'workflow_button';
  if ((column.interactionButtons?.length ?? 0) > 0)
    return 'interaction_buttons';
  return 'value';
}

/** Materialize an inferred action type onto a column that omits `type`.
 *  Plain value columns are returned untouched so definitions authored
 *  without an explicit `type` stay byte-identical. */
function normalizeTableColumn(column: ReportTableColumn): ReportTableColumn {
  if (column.type) return column;
  const inferred = inferReportTableColumnType(column);
  return inferred === 'value' ? column : { ...column, type: inferred };
}

function normalizeBlock(block: ReportBlockDefinition): ReportBlockDefinition {
  const columns = block.table?.columns;
  if (!columns || columns.length === 0) return block;
  const nextColumns = columns.map(normalizeTableColumn);
  const changed = nextColumns.some(
    (column, index) => column !== columns[index]
  );
  if (!changed) return block;
  return { ...block, table: { ...block.table, columns: nextColumns } };
}

/**
 * Normalize a definition on its way into `UpdateReportRequest.definition` /
 * `CreateReportRequest.definition`. Purely additive and idempotent: it only
 * writes back inference the readers already perform, and returns the input
 * unchanged when there is nothing to materialize.
 */
export function normalizeReportDefinitionForSave(
  definition: ReportDefinition
): ReportDefinition {
  const nextBlocks = definition.blocks.map(normalizeBlock);
  const changed = nextBlocks.some(
    (block, index) => block !== definition.blocks[index]
  );
  return changed ? { ...definition, blocks: nextBlocks } : definition;
}
