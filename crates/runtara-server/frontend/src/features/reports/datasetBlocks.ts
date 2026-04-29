import {
  ReportBlockDatasetQuery,
  ReportBlockDefinition,
  ReportDatasetDefinition,
  ReportOrderBy,
  ReportTableColumn,
} from './types';
import { humanizeFieldName } from './utils';

const DEFAULT_DATASET_BLOCK_LIMIT = 100;
const DEFAULT_TABLE_PAGE_SIZES = [25, 50, 100];

export function createDefaultDatasetBlockQuery(
  dataset: ReportDatasetDefinition,
  current?: ReportBlockDatasetQuery
): ReportBlockDatasetQuery {
  const dimensionFields = new Set(
    dataset.dimensions.map((dimension) => dimension.field)
  );
  const measureIds = new Set(dataset.measures.map((measure) => measure.id));
  const validDimensions = keepValidValues(current?.dimensions, dimensionFields);
  const validMeasures = keepValidValues(current?.measures, measureIds);
  const dimensions =
    validDimensions.length > 0
      ? validDimensions
      : dataset.dimensions[0]?.field
        ? [dataset.dimensions[0].field]
        : [];
  const measures =
    validMeasures.length > 0
      ? validMeasures
      : dataset.measures[0]?.id
        ? [dataset.measures[0].id]
        : [];
  const selectedFields = new Set([...dimensions, ...measures]);
  const orderBy = sanitizeOrderBy(current?.orderBy, selectedFields);

  return {
    id: dataset.id,
    dimensions,
    measures,
    orderBy:
      orderBy.length > 0
        ? orderBy
        : measures[0]
          ? [{ field: measures[0], direction: 'desc' }]
          : [],
    datasetFilters: (current?.datasetFilters ?? []).filter((filter) =>
      dimensionFields.has(filter.field)
    ),
    limit: Math.max(1, current?.limit ?? DEFAULT_DATASET_BLOCK_LIMIT),
  };
}

export function reconcileDatasetBlock(
  block: ReportBlockDefinition,
  dataset: ReportDatasetDefinition,
  query: ReportBlockDatasetQuery | undefined = block.dataset
): ReportBlockDefinition {
  const nextQuery = createDefaultDatasetBlockQuery(dataset, query);
  const outputFields = datasetQueryOutputFields(nextQuery);
  const source = { schema: '' };

  if (block.type === 'table') {
    return {
      ...block,
      source,
      dataset: nextQuery,
      table: {
        ...block.table,
        columns: outputFields.map((field) => datasetTableColumn(dataset, field)),
        defaultSort: sanitizeOrderBy(block.table?.defaultSort, new Set(outputFields)),
        pagination: block.table?.pagination ?? {
          defaultPageSize: 50,
          allowedPageSizes: DEFAULT_TABLE_PAGE_SIZES,
        },
      },
    };
  }

  if (block.type === 'chart') {
    const x =
      outputFields.includes(block.chart?.x ?? '')
        ? block.chart?.x ?? ''
        : nextQuery.dimensions?.[0] ?? outputFields[0] ?? '';
    return {
      ...block,
      source,
      dataset: nextQuery,
      chart: {
        kind: block.chart?.kind ?? 'bar',
        x,
        series: (nextQuery.measures ?? []).map((field) => ({
          field,
          label: datasetFieldLabel(dataset, field),
        })),
      },
    };
  }

  if (block.type === 'metric') {
    const valueField =
      nextQuery.measures?.[0] ?? nextQuery.dimensions?.[0] ?? outputFields[0] ?? '';
    return {
      ...block,
      source,
      dataset: nextQuery,
      metric: {
        valueField,
        label: datasetFieldLabel(dataset, valueField),
        format: datasetFieldFormat(dataset, valueField),
      },
    };
  }

  return {
    ...block,
    source,
    dataset: nextQuery,
  };
}

export function datasetQueryOutputFields(
  query: ReportBlockDatasetQuery
): string[] {
  return [...(query.dimensions ?? []), ...(query.measures ?? [])];
}

export function datasetFieldLabel(
  dataset: ReportDatasetDefinition | undefined,
  field: string
): string {
  return (
    dataset?.dimensions.find((dimension) => dimension.field === field)?.label ??
    dataset?.measures.find((measure) => measure.id === field)?.label ??
    humanizeFieldName(field)
  );
}

function datasetFieldFormat(
  dataset: ReportDatasetDefinition,
  field: string
): string | undefined {
  return (
    dataset.dimensions.find((dimension) => dimension.field === field)?.format ??
    dataset.measures.find((measure) => measure.id === field)?.format
  );
}

function datasetTableColumn(
  dataset: ReportDatasetDefinition,
  field: string
): ReportTableColumn {
  return {
    field,
    label: datasetFieldLabel(dataset, field),
    format: datasetFieldFormat(dataset, field),
  };
}

function keepValidValues(
  values: string[] | undefined,
  validValues: Set<string>
): string[] {
  return (values ?? []).filter((value) => validValues.has(value));
}

function sanitizeOrderBy(
  orderBy: ReportOrderBy[] | undefined,
  validFields: Set<string>
): ReportOrderBy[] {
  return (orderBy ?? []).filter((sort) => validFields.has(sort.field));
}
