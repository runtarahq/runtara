import { DragEvent, ReactNode, useMemo, useState } from 'react';
import {
  Copy,
  CreditCard,
  GripVertical,
  LineChart,
  Plus,
  Rows3,
  Sigma,
  Text,
  Trash2,
  Wrench,
} from 'lucide-react';
import { Schema } from '@/generated/RuntaraRuntimeApi';
import { cn } from '@/lib/utils';
import { Badge } from '@/shared/components/ui/badge';
import { Button } from '@/shared/components/ui/button';
import { Checkbox } from '@/shared/components/ui/checkbox';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { Separator } from '@/shared/components/ui/separator';
import { Switch } from '@/shared/components/ui/switch';
import { Textarea } from '@/shared/components/ui/textarea';
import {
  ReportAggregateFn,
  ReportBlockDatasetQuery,
  ReportBlockDefinition,
  ReportBlockType,
  ReportChartKind,
  ReportDatasetFilterRequest,
  ReportDatasetDefinition,
  ReportDefinition,
  ReportFilterDefinition,
  ReportFilterType,
  ReportLayoutNode,
  ReportSourceJoin,
  ReportTableColumn,
} from '../types';
import {
  createDefaultDatasetBlockQuery,
  datasetFieldLabel,
  datasetQueryOutputFields,
  reconcileDatasetBlock,
} from '../datasetBlocks';
import {
  extractLayoutBlockReferences,
  humanizeFieldName,
  slugify,
} from '../utils';

type ReportDefinitionBuilderProps = {
  value: ReportDefinition;
  schemas: Schema[];
  selectedSchema: string;
  onSelectedSchemaChange: (schema: string) => void;
  onChange: (definition: ReportDefinition) => void;
};

type EditorNode =
  | {
      kind: 'markdown';
      nodeId: string;
      layoutId?: string;
      blockId: string;
      content: string;
    }
  | {
      kind: 'block';
      nodeId: string;
      layoutId?: string;
      blockId: string;
    }
  | {
      kind: 'metric_row';
      nodeId: string;
      layoutId?: string;
      title?: string;
      blocks: string[];
    }
  | {
      kind: 'layout';
      nodeId: string;
      layout: ReportLayoutNode;
    };

type BlockEditorProps = {
  block: ReportBlockDefinition | undefined;
  blockId: string;
  schemas: Schema[];
  datasets: ReportDatasetDefinition[];
  onChange: (block: ReportBlockDefinition) => void;
  onDuplicate: () => void;
  onRemove: () => void;
  onCreateMissing: () => void;
};

const NONE_VALUE = '__none__';

const BLOCK_TYPE_META: Record<
  Exclude<ReportBlockType, 'markdown'>,
  { label: string; icon: typeof Rows3 }
> = {
  table: { label: 'Table', icon: Rows3 },
  metric: { label: 'Metric', icon: Sigma },
  chart: { label: 'Chart', icon: LineChart },
  actions: { label: 'Actions', icon: Wrench },
  card: { label: 'Card', icon: CreditCard },
};

const AGGREGATE_OPTIONS: Array<{
  label: string;
  value: ReportAggregateFn;
}> = [
  { label: 'Count', value: 'count' },
  { label: 'Sum', value: 'sum' },
  { label: 'Average', value: 'avg' },
  { label: 'Minimum', value: 'min' },
  { label: 'Maximum', value: 'max' },
  { label: 'First value', value: 'first_value' },
  { label: 'Last value', value: 'last_value' },
];

const FILTER_TYPE_OPTIONS: Array<{
  label: string;
  value: ReportFilterType;
}> = [
  { label: 'Select', value: 'select' },
  { label: 'Multi-select', value: 'multi_select' },
  { label: 'Radio', value: 'radio' },
  { label: 'Time range', value: 'time_range' },
  { label: 'Number range', value: 'number_range' },
  { label: 'Text', value: 'text' },
  { label: 'Search', value: 'search' },
  { label: 'Checkbox', value: 'checkbox' },
];

const CHART_KIND_OPTIONS: Array<{
  label: string;
  value: ReportChartKind;
}> = [
  { label: 'Line', value: 'line' },
  { label: 'Bar', value: 'bar' },
  { label: 'Area', value: 'area' },
  { label: 'Pie', value: 'pie' },
  { label: 'Donut', value: 'donut' },
];

const COLUMN_FORMAT_OPTIONS = [
  { label: 'Default', value: NONE_VALUE },
  { label: 'Number', value: 'number' },
  { label: 'Decimal', value: 'decimal' },
  { label: 'Currency', value: 'currency' },
  { label: 'Percent', value: 'percent' },
  { label: 'Date', value: 'date' },
  { label: 'Datetime', value: 'datetime' },
  { label: 'Pill', value: 'pill' },
  { label: 'Bar indicator', value: 'bar_indicator' },
  { label: 'JSON', value: 'json' },
  { label: 'Markdown', value: 'markdown' },
];

const ALIGN_OPTIONS = [
  { label: 'Left', value: 'left' },
  { label: 'Center', value: 'center' },
  { label: 'Right', value: 'right' },
];

const JOIN_KIND_OPTIONS = [
  { label: 'Left join', value: 'left' },
  { label: 'Inner join', value: 'inner' },
];

const CONDITION_OPERATOR_OPTIONS = [
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

export function ReportDefinitionBuilder({
  value,
  schemas,
  selectedSchema,
  onSelectedSchemaChange,
  onChange,
}: ReportDefinitionBuilderProps) {
  const [dragIndex, setDragIndex] = useState<number | null>(null);
  const [dropIndex, setDropIndex] = useState<number | null>(null);
  const nodes = useMemo(() => definitionToNodes(value), [value]);
  const defaultSchema = selectedSchema || schemas[0]?.name || '';

  const commitNodes = (
    nextNodes: EditorNode[],
    nextBlocks: ReportBlockDefinition[] = value.blocks
  ) => {
    onChange(nodesToDefinition(value, nextNodes, nextBlocks));
  };

  const updateMarkdownNode = (nodeIndex: number, content: string) => {
    const nextNodes = nodes.map((node, index) =>
      index === nodeIndex && node.kind === 'markdown'
        ? { ...node, content }
        : node
    );
    commitNodes(nextNodes);
  };

  const updateBlock = (blockId: string, block: ReportBlockDefinition) => {
    const nextBlocks = value.blocks.map((current) =>
      current.id === blockId ? block : current
    );
    const nextNodes = replaceBlockIdInNodes(nodes, blockId, block.id);
    commitNodes(nextNodes, nextBlocks);
  };

  const addMarkdownAfter = (nodeIndex: number) => {
    const block = createMarkdownBlock(
      uniqueBlockId(value.blocks, 'markdown'),
      '## New section'
    );
    const nextNodes = insertAfter(nodes, nodeIndex, {
      kind: 'markdown',
      nodeId: `block-${block.id}`,
      layoutId: uniqueLayoutNodeId(value.layout ?? [], `${block.id}_node`),
      blockId: block.id,
      content: block.markdown?.content ?? '',
    });
    commitNodes(nextNodes, [...value.blocks, block]);
  };

  const addBlockAfter = (
    nodeIndex: number,
    blockType: Exclude<ReportBlockType, 'markdown'>
  ) => {
    const block = createDefaultBlock(blockType, defaultSchema, value.blocks);
    const nextNodes = insertAfter(nodes, nodeIndex, {
      kind: 'block',
      nodeId: `block-${block.id}`,
      layoutId: uniqueLayoutNodeId(value.layout ?? [], `${block.id}_node`),
      blockId: block.id,
    });
    commitNodes(nextNodes, [...value.blocks, block]);
  };

  const addMetricRowAfter = (nodeIndex: number) => {
    const existingMetricIds = value.blocks
      .filter((block) => block.type === 'metric')
      .map((block) => block.id);
    const needsMetric = existingMetricIds.length === 0;
    const block = needsMetric
      ? createDefaultBlock('metric', defaultSchema, value.blocks)
      : null;
    const metricIds = block ? [block.id] : existingMetricIds.slice(0, 3);
    const nextNodes = insertAfter(nodes, nodeIndex, {
      kind: 'metric_row',
      nodeId: `metric-row-${Date.now()}`,
      layoutId: uniqueLayoutNodeId(value.layout ?? [], 'metric_row'),
      blocks: metricIds,
    });
    commitNodes(nextNodes, block ? [...value.blocks, block] : value.blocks);
  };

  const appendBlock = (blockType: Exclude<ReportBlockType, 'markdown'>) => {
    const block = createDefaultBlock(blockType, defaultSchema, value.blocks);
    commitNodes(
      [
        ...nodes,
        {
          kind: 'block',
          nodeId: `block-${block.id}`,
          layoutId: uniqueLayoutNodeId(value.layout ?? [], `${block.id}_node`),
          blockId: block.id,
        },
      ],
      [...value.blocks, block]
    );
  };

  const appendMetricRow = () => {
    addMetricRowAfter(nodes.length - 1);
  };

  const duplicateBlock = (blockId: string) => {
    const block = value.blocks.find((candidate) => candidate.id === blockId);
    if (!block) return;
    const copy = duplicateBlockDefinition(block, value.blocks);
    const sourceIndex = nodes.findIndex(
      (node) => node.kind === 'block' && node.blockId === blockId
    );
    const nextNodes = insertAfter(nodes, sourceIndex, {
      kind: 'block',
      nodeId: `block-${copy.id}`,
      layoutId: uniqueLayoutNodeId(value.layout ?? [], `${copy.id}_node`),
      blockId: copy.id,
    });
    commitNodes(nextNodes, [...value.blocks, copy]);
  };

  const removeNode = (nodeIndex: number) => {
    const node = nodes[nodeIndex];
    const nextNodes = nodes.filter((_, index) => index !== nodeIndex);
    const nextBlocks =
      node?.kind === 'block' || node?.kind === 'markdown'
        ? value.blocks.filter((block) => block.id !== node.blockId)
        : value.blocks;
    commitNodes(nextNodes, nextBlocks);
  };

  const updateMetricRowNode = (
    nodeIndex: number,
    patch: Partial<Extract<EditorNode, { kind: 'metric_row' }>>
  ) => {
    const nextNodes = nodes.map((node, index) =>
      index === nodeIndex && node.kind === 'metric_row'
        ? { ...node, ...patch }
        : node
    );
    commitNodes(nextNodes);
  };

  const createMissingBlock = (blockId: string) => {
    const block = createDefaultBlock('table', defaultSchema, value.blocks);
    commitNodes(nodes, [
      ...value.blocks,
      { ...block, id: blockId, title: humanizeFieldName(blockId) },
    ]);
  };

  const moveNode = (fromIndex: number, toIndex: number) => {
    if (fromIndex === toIndex) return;
    commitNodes(moveItem(nodes, fromIndex, toIndex));
  };

  const handleDragStart = (event: DragEvent, index: number) => {
    setDragIndex(index);
    event.dataTransfer.effectAllowed = 'move';
    event.dataTransfer.setData('text/plain', String(index));
  };

  const handleDragOver = (event: DragEvent, index: number) => {
    event.preventDefault();
    event.dataTransfer.dropEffect = 'move';
    setDropIndex(index);
  };

  const handleDrop = (event: DragEvent, index: number) => {
    event.preventDefault();
    const sourceIndex = Number(event.dataTransfer.getData('text/plain'));
    if (Number.isInteger(sourceIndex)) {
      moveNode(sourceIndex, index);
    }
    setDragIndex(null);
    setDropIndex(null);
  };

  return (
    <section className="flex flex-col gap-4 rounded-lg border bg-background p-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex flex-wrap items-center gap-3">
          <Badge variant="secondary">{value.blocks.length} blocks</Badge>
          <div className="flex min-w-64 flex-col gap-1">
            <Label className="text-xs text-muted-foreground">
              Schema for new raw blocks
            </Label>
            <Select
              value={selectedSchema}
              onValueChange={onSelectedSchemaChange}
            >
              <SelectTrigger className="h-9">
                <SelectValue placeholder="Choose Object Model schema" />
              </SelectTrigger>
              <SelectContent>
                {schemas.map((schema) => (
                  <SelectItem key={schema.id} value={schema.name}>
                    {schema.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => appendBlock('table')}
          >
            <Rows3 className="mr-2 size-4" />
            Raw table
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => appendBlock('metric')}
          >
            <Sigma className="mr-2 size-4" />
            Raw metric
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => appendBlock('chart')}
          >
            <LineChart className="mr-2 size-4" />
            Raw chart
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={appendMetricRow}
          >
            <Rows3 className="mr-2 size-4" />
            Metric row
          </Button>
        </div>
      </div>

      <div className="flex flex-col gap-3">
        {nodes.map((node, index) => (
          <div
            key={node.nodeId}
            className={cn(
              'group rounded-lg border border-transparent transition-colors',
              dropIndex === index && dragIndex !== index && 'border-primary'
            )}
            onDragOver={(event) => handleDragOver(event, index)}
            onDrop={(event) => handleDrop(event, index)}
          >
            <div className="grid grid-cols-[2.25rem_minmax(0,1fr)] gap-2">
              <div className="flex flex-col items-center gap-1 pt-3">
                <button
                  type="button"
                  className="flex size-8 cursor-grab items-center justify-center rounded-md text-muted-foreground hover:bg-muted active:cursor-grabbing"
                  draggable
                  aria-label="Move block"
                  onDragStart={(event) => handleDragStart(event, index)}
                  onDragEnd={() => {
                    setDragIndex(null);
                    setDropIndex(null);
                  }}
                >
                  <GripVertical className="size-4" />
                </button>
                <AddNodeMenu
                  onAddMarkdown={() => addMarkdownAfter(index)}
                  onAddBlock={(type) => addBlockAfter(index, type)}
                  onAddMetricRow={() => addMetricRowAfter(index)}
                />
              </div>
              {node.kind === 'markdown' ? (
                <MarkdownNodeEditor
                  content={node.content}
                  onChange={(content) => updateMarkdownNode(index, content)}
                  onRemove={() => removeNode(index)}
                />
              ) : node.kind === 'block' ? (
                <ReportBlockEditor
                  block={value.blocks.find(
                    (block) => block.id === node.blockId
                  )}
                  blockId={node.blockId}
                  schemas={schemas}
                  datasets={value.datasets ?? []}
                  onChange={(block) => updateBlock(node.blockId, block)}
                  onDuplicate={() => duplicateBlock(node.blockId)}
                  onRemove={() => removeNode(index)}
                  onCreateMissing={() => createMissingBlock(node.blockId)}
                />
              ) : node.kind === 'metric_row' ? (
                <MetricRowNodeEditor
                  node={node}
                  blocks={value.blocks}
                  onChange={(patch) => updateMetricRowNode(index, patch)}
                  onRemove={() => removeNode(index)}
                />
              ) : (
                <LayoutNodeSummary
                  node={node.layout}
                  onRemove={() => removeNode(index)}
                />
              )}
            </div>
          </div>
        ))}
        {nodes.length === 0 && (
          <div className="flex min-h-48 flex-col items-center justify-center gap-3 rounded-lg border border-dashed bg-muted/20 p-8">
            <Button type="button" onClick={() => appendBlock('table')}>
              <Plus className="mr-2 size-4" />
              Add block
            </Button>
          </div>
        )}
      </div>
    </section>
  );
}

function AddNodeMenu({
  onAddMarkdown,
  onAddBlock,
  onAddMetricRow,
}: {
  onAddMarkdown: () => void;
  onAddBlock: (type: Exclude<ReportBlockType, 'markdown'>) => void;
  onAddMetricRow: () => void;
}) {
  return (
    <div className="flex flex-col gap-1 opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100">
      <Button
        type="button"
        variant="ghost"
        size="icon"
        className="size-8"
        onClick={onAddMarkdown}
        aria-label="Add text"
      >
        <Text className="size-4" />
      </Button>
      <Button
        type="button"
        variant="ghost"
        size="icon"
        className="size-8"
        onClick={() => onAddBlock('table')}
        aria-label="Add table"
      >
        <Rows3 className="size-4" />
      </Button>
      <Button
        type="button"
        variant="ghost"
        size="icon"
        className="size-8"
        onClick={onAddMetricRow}
        aria-label="Add metric row"
      >
        <Sigma className="size-4" />
      </Button>
    </div>
  );
}

function MarkdownNodeEditor({
  content,
  onChange,
  onRemove,
}: {
  content: string;
  onChange: (content: string) => void;
  onRemove: () => void;
}) {
  return (
    <div className="flex flex-col gap-2 rounded-lg border bg-muted/10 p-3">
      <div className="flex items-center justify-between gap-2">
        <Badge variant="muted">Text</Badge>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="size-8"
          onClick={onRemove}
          aria-label="Remove text"
        >
          <Trash2 className="size-4" />
        </Button>
      </div>
      <Textarea
        value={content}
        className="min-h-32 resize-y border-0 bg-transparent p-0 text-sm shadow-none focus-visible:ring-0"
        onChange={(event) => onChange(event.target.value)}
      />
    </div>
  );
}

function MetricRowNodeEditor({
  node,
  blocks,
  onChange,
  onRemove,
}: {
  node: Extract<EditorNode, { kind: 'metric_row' }>;
  blocks: ReportBlockDefinition[];
  onChange: (
    patch: Partial<Extract<EditorNode, { kind: 'metric_row' }>>
  ) => void;
  onRemove: () => void;
}) {
  const metricBlocks = blocks.filter((block) => block.type === 'metric');
  const selected = new Set(node.blocks);

  return (
    <div className="flex flex-col gap-3 rounded-lg border bg-background p-4">
      <div className="flex items-center justify-between gap-2">
        <Badge variant="secondary">Metric row</Badge>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="size-8"
          onClick={onRemove}
          aria-label="Remove metric row"
        >
          <Trash2 className="size-4" />
        </Button>
      </div>
      <Field label="Title">
        <Input
          value={node.title ?? ''}
          placeholder="Optional row title"
          onChange={(event) => onChange({ title: event.target.value })}
        />
      </Field>
      <div className="grid gap-2 sm:grid-cols-2 xl:grid-cols-3">
        {metricBlocks.map((block) => (
          <label
            key={block.id}
            className="flex min-h-10 items-center gap-2 rounded-md border px-3 py-2 text-sm"
          >
            <Checkbox
              checked={selected.has(block.id)}
              onCheckedChange={(checked) => {
                const nextBlocks = checked
                  ? [...node.blocks, block.id]
                  : node.blocks.filter((blockId) => blockId !== block.id);
                onChange({ blocks: nextBlocks });
              }}
            />
            <span className="truncate">
              {block.title || humanizeFieldName(block.id)}
            </span>
          </label>
        ))}
      </div>
      {metricBlocks.length === 0 && (
        <div className="rounded-md border border-dashed bg-muted/20 p-4 text-sm text-muted-foreground" />
      )}
    </div>
  );
}

function LayoutNodeSummary({
  node,
  onRemove,
}: {
  node: ReportLayoutNode;
  onRemove: () => void;
}) {
  return (
    <div className="flex items-center justify-between gap-3 rounded-lg border bg-muted/10 p-4">
      <div className="min-w-0">
        <Badge variant="secondary">Layout</Badge>
        <p className="mt-2 truncate text-sm font-semibold text-foreground">
          {node.id}
        </p>
        <p className="text-xs text-muted-foreground">{node.type}</p>
      </div>
      <Button
        type="button"
        variant="ghost"
        size="icon"
        className="size-8"
        onClick={onRemove}
        aria-label="Remove layout node"
      >
        <Trash2 className="size-4" />
      </Button>
    </div>
  );
}

function ReportBlockEditor({
  block,
  blockId,
  schemas,
  datasets,
  onChange,
  onDuplicate,
  onRemove,
  onCreateMissing,
}: BlockEditorProps) {
  if (!block) {
    return (
      <div className="flex items-center justify-between gap-3 rounded-lg border border-destructive/30 bg-destructive/5 p-4">
        <div className="min-w-0">
          <p className="truncate text-sm font-semibold text-destructive">
            Missing block: {blockId}
          </p>
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={onCreateMissing}
        >
          Create
        </Button>
      </div>
    );
  }

  const blockType =
    block.type === 'markdown'
      ? 'table'
      : (block.type as Exclude<ReportBlockType, 'markdown'>);
  const blockMeta = BLOCK_TYPE_META[blockType];
  const TypeIcon = blockMeta.icon;
  const source = block.source ?? emptyReportSource();
  const dataset = block.dataset
    ? datasets.find((candidate) => candidate.id === block.dataset?.id)
    : undefined;
  const schemaName = source.schema || dataset?.source.schema || '';
  const schema = schemas.find((candidate) => candidate.name === schemaName);
  const baseFields = getSchemaFields(schema);
  const fields = getBlockAvailableFields(schema, source.join, schemas);
  const isDatasetBlock = Boolean(block.dataset);
  const isWorkflowRuntimeBlock = source.kind === 'workflow_runtime';

  const update = (patch: Partial<ReportBlockDefinition>) => {
    onChange({ ...block, ...patch });
  };

  const updateType = (type: Exclude<ReportBlockType, 'markdown'>) => {
    onChange(convertBlockType(block, type, fields));
  };

  const updateDataset = (datasetId: string) => {
    const nextDataset = datasets.find(
      (candidate) => candidate.id === datasetId
    );
    if (!nextDataset) return;
    onChange(
      reconcileDatasetBlock(
        block,
        nextDataset,
        createDefaultDatasetBlockQuery(nextDataset)
      )
    );
  };

  return (
    <div className="flex flex-col gap-4 rounded-lg border bg-background p-4">
      <div className="flex flex-col gap-3 lg:flex-row lg:items-start lg:justify-between">
        <div className="flex min-w-0 flex-1 items-start gap-3">
          <div className="flex size-10 shrink-0 items-center justify-center rounded-md bg-muted text-muted-foreground">
            <TypeIcon className="size-5" />
          </div>
          <div className="flex min-w-0 flex-1 flex-col gap-2">
            <Input
              value={block.title ?? ''}
              placeholder={blockMeta.label}
              className="h-9 border-0 px-0 text-base font-semibold shadow-none focus-visible:ring-0"
              onChange={(event) => update({ title: event.target.value })}
            />
            <div className="flex flex-wrap items-center gap-2">
              <Badge variant="secondary">{blockMeta.label}</Badge>
              <Badge variant="outline">
                {isDatasetBlock
                  ? (dataset?.label ?? block.dataset?.id ?? 'Dataset')
                  : isWorkflowRuntimeBlock
                    ? (source.entity ?? 'Workflow runtime')
                    : source.schema || 'No schema'}
              </Badge>
              {isDatasetBlock && schemaName && (
                <Badge variant="outline">Source: {schemaName}</Badge>
              )}
              {block.lazy && <Badge variant="muted">Lazy</Badge>}
            </div>
          </div>
        </div>
        <div className="flex shrink-0 flex-wrap gap-2">
          {isDatasetBlock ? (
            <Badge variant="outline" className="h-9 px-3">
              Dataset block
            </Badge>
          ) : isWorkflowRuntimeBlock ? (
            <Badge variant="outline" className="h-9 px-3">
              Workflow runtime
            </Badge>
          ) : (
            <Select
              value={blockType}
              onValueChange={(value) =>
                updateType(value as Exclude<ReportBlockType, 'markdown'>)
              }
            >
              <SelectTrigger className="h-9 w-32">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {Object.entries(BLOCK_TYPE_META)
                  .filter(([type]) => type !== 'actions')
                  .map(([type, meta]) => (
                    <SelectItem key={type} value={type}>
                      {meta.label}
                    </SelectItem>
                  ))}
              </SelectContent>
            </Select>
          )}
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="size-9"
            onClick={onDuplicate}
            aria-label="Duplicate block"
          >
            <Copy className="size-4" />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="size-9"
            onClick={onRemove}
            aria-label="Remove block"
          >
            <Trash2 className="size-4" />
          </Button>
        </div>
      </div>

      <div className="grid gap-3 md:grid-cols-3">
        <Field label="Block ID">
          <Input
            value={block.id}
            onChange={(event) =>
              update({ id: slugify(event.target.value).replace(/-/g, '_') })
            }
          />
        </Field>
        {isDatasetBlock ? (
          <Field label="Dataset">
            <Select
              value={dataset?.id ?? block.dataset?.id ?? NONE_VALUE}
              disabled={datasets.length === 0}
              onValueChange={updateDataset}
            >
              <SelectTrigger>
                <SelectValue placeholder="Select dataset" />
              </SelectTrigger>
              <SelectContent>
                {!dataset && block.dataset?.id && (
                  <SelectItem value={block.dataset.id} disabled>
                    Missing dataset: {block.dataset.id}
                  </SelectItem>
                )}
                {datasets.map((datasetOption) => (
                  <SelectItem key={datasetOption.id} value={datasetOption.id}>
                    {datasetOption.label || datasetOption.id}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </Field>
        ) : isWorkflowRuntimeBlock ? (
          <>
            <Field label="Workflow ID">
              <Input
                value={source.workflowId ?? ''}
                onChange={(event) =>
                  update({
                    source: {
                      ...source,
                      workflowId: event.target.value,
                    },
                  })
                }
              />
            </Field>
            <Field label="Entity">
              <Select
                value={source.entity ?? 'instances'}
                onValueChange={(entity) =>
                  update({
                    source: {
                      ...source,
                      entity: entity as 'instances' | 'actions',
                    },
                  })
                }
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="instances">Instances</SelectItem>
                  <SelectItem value="actions">Actions</SelectItem>
                </SelectContent>
              </Select>
            </Field>
            <Field label="Instance ID">
              <Input
                value={source.instanceId ?? ''}
                onChange={(event) =>
                  update({
                    source: {
                      ...source,
                      instanceId: event.target.value || undefined,
                    },
                  })
                }
              />
            </Field>
          </>
        ) : (
          <Field label="Schema">
            <Select
              value={source.schema}
              onValueChange={(schemaName) =>
                onChange(changeBlockSchema(block, schemaName, schemas))
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
          </Field>
        )}
        <div className="flex items-center justify-between gap-3 rounded-md border px-3 py-2">
          <div>
            <Label className="text-sm">Lazy load</Label>
          </div>
          <Switch
            checked={Boolean(block.lazy)}
            onCheckedChange={(checked) => update({ lazy: checked })}
          />
        </div>
      </div>

      {!isDatasetBlock &&
        !isWorkflowRuntimeBlock &&
        source.kind !== 'system' && (
          <SourceJoinsEditor
            block={block}
            schemas={schemas}
            baseFields={baseFields}
            onChange={onChange}
          />
        )}

      <Separator />

      {isDatasetBlock ? (
        <DatasetBlockSettings
          block={block}
          dataset={dataset}
          onChange={onChange}
        />
      ) : (
        <>
          {blockType === 'table' && (
            <TableBlockSettings
              block={block}
              fields={fields}
              onChange={onChange}
            />
          )}
          {blockType === 'metric' && (
            <MetricBlockSettings
              block={block}
              fields={fields}
              onChange={onChange}
            />
          )}
          {blockType === 'chart' && (
            <ChartBlockSettings
              block={block}
              fields={fields}
              onChange={onChange}
            />
          )}
          {blockType === 'card' && (
            <CardBlockSettings
              block={block}
              fields={fields}
              onChange={onChange}
            />
          )}
        </>
      )}

      {!isDatasetBlock && (blockType === 'table' || blockType === 'chart') && (
        <>
          <Separator />
          <BlockFiltersEditor
            block={block}
            fields={fields}
            onChange={onChange}
          />
        </>
      )}
    </div>
  );
}

function DatasetBlockSettings({
  block,
  dataset,
  onChange,
}: {
  block: ReportBlockDefinition;
  dataset: ReportDatasetDefinition | undefined;
  onChange: (block: ReportBlockDefinition) => void;
}) {
  const query = block.dataset;
  if (!query) return null;

  if (!dataset) {
    return (
      <div className="rounded-md border border-destructive/30 bg-destructive/5 p-4 text-sm text-destructive">
        This block references missing dataset "{query.id}".
      </div>
    );
  }

  const dimensions = query.dimensions ?? [];
  const measures = query.measures ?? [];
  const selectedDimensions = new Set(dimensions);
  const selectedMeasures = new Set(measures);
  const outputFields = datasetQueryOutputFields(query);
  const sort = query.orderBy?.[0];
  const datasetFilters = query.datasetFilters ?? [];
  const datasetFilterFields = datasetFilterableFields(dataset);

  const updateQuery = (nextQuery: ReportBlockDatasetQuery) => {
    onChange(reconcileDatasetBlock(block, dataset, nextQuery));
  };

  const updateBlock = (patch: Partial<ReportBlockDefinition>) => {
    onChange({ ...block, ...patch });
  };

  const updateDatasetFilter = (
    index: number,
    patch: Partial<ReportDatasetFilterRequest>
  ) => {
    updateQuery({
      ...query,
      datasetFilters: datasetFilters.map((filter, currentIndex) =>
        currentIndex === index ? { ...filter, ...patch } : filter
      ),
    });
  };

  const addDatasetFilter = () => {
    updateQuery({
      ...query,
      datasetFilters: [
        ...datasetFilters,
        {
          field: datasetFilterFields[0] ?? outputFields[0] ?? '',
          op: 'eq',
          value: '',
        },
      ],
    });
  };

  return (
    <div className="flex flex-col gap-4">
      <div className="grid gap-3 md:grid-cols-2">
        <Field label="Dataset ID">
          <Input value={query.id} readOnly className="bg-muted/40" />
        </Field>
        <Field label="Source schema">
          <Input
            value={dataset.source.schema}
            readOnly
            className="bg-muted/40"
          />
        </Field>
        <Field label="Sort">
          <Select
            value={sort?.field ?? NONE_VALUE}
            onValueChange={(field) =>
              updateQuery({
                ...query,
                orderBy:
                  field === NONE_VALUE
                    ? []
                    : [
                        {
                          field,
                          direction: sort?.direction ?? 'desc',
                        },
                      ],
              })
            }
          >
            <SelectTrigger>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={NONE_VALUE}>No explicit sort</SelectItem>
              {outputFields.map((field) => (
                <SelectItem key={field} value={field}>
                  {datasetFieldLabel(dataset, field)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Field>
        <Field label="Sort direction">
          <Select
            value={sort?.direction ?? 'desc'}
            disabled={!sort}
            onValueChange={(direction) =>
              updateQuery({
                ...query,
                orderBy: sort ? [{ ...sort, direction }] : [],
              })
            }
          >
            <SelectTrigger>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="desc">Descending</SelectItem>
              <SelectItem value="asc">Ascending</SelectItem>
            </SelectContent>
          </Select>
        </Field>
        <Field label="Limit">
          <Input
            type="number"
            min={1}
            value={String(query.limit ?? 100)}
            onChange={(event) =>
              updateQuery({
                ...query,
                limit: Math.max(1, Number(event.target.value) || 100),
              })
            }
          />
        </Field>
      </div>

      <div className="flex flex-col gap-3 rounded-md border bg-muted/10 p-3">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="flex items-center gap-2">
            <Label>Dataset filters</Label>
            <Badge variant="secondary">{datasetFilters.length} filters</Badge>
          </div>
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={datasetFilterFields.length === 0}
            onClick={addDatasetFilter}
          >
            <Plus className="mr-2 size-4" />
            Add filter
          </Button>
        </div>
        {datasetFilters.length === 0 ? (
          <div className="rounded-md border border-dashed bg-background p-3 text-sm text-muted-foreground">
            No fixed dataset filters. Use these for block-specific constraints
            that should always apply before rendering.
          </div>
        ) : (
          <div className="flex flex-col gap-2">
            {datasetFilters.map((filter, index) => (
              <div
                key={`dataset-filter-${index}-${filter.field}`}
                className="grid gap-2 rounded-md border bg-background p-3 md:grid-cols-[minmax(0,1fr)_12rem_minmax(0,1fr)_40px]"
              >
                <Field label="Field">
                  <Select
                    value={filter.field || NONE_VALUE}
                    onValueChange={(field) =>
                      updateDatasetFilter(index, { field })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value={NONE_VALUE} disabled>
                        Select field
                      </SelectItem>
                      {datasetFilterFields.map((field) => (
                        <SelectItem key={field} value={field}>
                          {datasetFieldLabel(dataset, field)}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </Field>
                <Field label="Operator">
                  <Select
                    value={filter.op ?? 'eq'}
                    onValueChange={(op) => updateDatasetFilter(index, { op })}
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {CONDITION_OPERATOR_OPTIONS.map((option) => (
                        <SelectItem key={option.value} value={option.value}>
                          {option.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </Field>
                <Field label="Value">
                  <Input
                    value={formatDatasetFilterValue(filter.value, filter.op)}
                    placeholder={
                      filter.op === 'between'
                        ? '10..20'
                        : filter.op === 'in'
                          ? 'open, pending'
                          : 'Value'
                    }
                    onChange={(event) =>
                      updateDatasetFilter(index, {
                        value: parseDatasetFilterValue(
                          event.target.value,
                          filter.op
                        ),
                      })
                    }
                  />
                </Field>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="mt-6 size-9"
                  onClick={() =>
                    updateQuery({
                      ...query,
                      datasetFilters: datasetFilters.filter(
                        (_, currentIndex) => currentIndex !== index
                      ),
                    })
                  }
                  aria-label="Remove dataset filter"
                >
                  <Trash2 className="size-4" />
                </Button>
              </div>
            ))}
          </div>
        )}
      </div>

      <div className="grid gap-4 xl:grid-cols-2">
        <div className="space-y-2">
          <Label>Dimensions</Label>
          <div className="grid gap-2 sm:grid-cols-2">
            {dataset.dimensions.map((dimension) => (
              <label
                key={dimension.field}
                className="flex min-h-10 items-center gap-2 rounded-md border px-3 py-2 text-sm"
              >
                <Checkbox
                  checked={selectedDimensions.has(dimension.field)}
                  onCheckedChange={(checked) => {
                    const nextDimensions = checked
                      ? [...dimensions, dimension.field]
                      : dimensions.filter((field) => field !== dimension.field);
                    updateQuery({
                      ...query,
                      dimensions: nextDimensions,
                      orderBy: (query.orderBy ?? []).filter((item) =>
                        [...nextDimensions, ...measures].includes(item.field)
                      ),
                    });
                  }}
                />
                <span className="truncate">{dimension.label}</span>
              </label>
            ))}
          </div>
        </div>

        <div className="space-y-2">
          <Label>Measures</Label>
          <div className="grid gap-2 sm:grid-cols-2">
            {dataset.measures.map((measure) => (
              <label
                key={measure.id}
                className="flex min-h-10 items-center gap-2 rounded-md border px-3 py-2 text-sm"
              >
                <Checkbox
                  checked={selectedMeasures.has(measure.id)}
                  onCheckedChange={(checked) => {
                    const nextMeasures = checked
                      ? [...measures, measure.id]
                      : measures.filter((field) => field !== measure.id);
                    updateQuery({
                      ...query,
                      measures: nextMeasures,
                      orderBy: (query.orderBy ?? []).filter((item) =>
                        [...dimensions, ...nextMeasures].includes(item.field)
                      ),
                    });
                  }}
                />
                <span className="truncate">{measure.label}</span>
              </label>
            ))}
          </div>
        </div>
      </div>

      {block.type === 'chart' && (
        <div className="grid gap-3 md:grid-cols-2">
          <Field label="Chart type">
            <Select
              value={block.chart?.kind ?? 'bar'}
              onValueChange={(kind) =>
                updateBlock({
                  chart: {
                    kind: kind as ReportChartKind,
                    x: block.chart?.x ?? dimensions[0] ?? outputFields[0] ?? '',
                    series: block.chart?.series ?? [],
                  },
                })
              }
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {CHART_KIND_OPTIONS.map((option) => (
                  <SelectItem key={option.value} value={option.value}>
                    {option.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </Field>
          <Field label="X-axis">
            <Select
              value={block.chart?.x ?? dimensions[0] ?? NONE_VALUE}
              onValueChange={(x) =>
                updateBlock({
                  chart: {
                    kind: block.chart?.kind ?? 'bar',
                    x,
                    series: block.chart?.series ?? [],
                  },
                })
              }
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {outputFields.map((field) => (
                  <SelectItem key={field} value={field}>
                    {datasetFieldLabel(dataset, field)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </Field>
        </div>
      )}

      {block.type === 'metric' && (
        <Field label="Metric value">
          <Select
            value={block.metric?.valueField ?? measures[0] ?? NONE_VALUE}
            onValueChange={(valueField) =>
              updateBlock({
                metric: {
                  valueField,
                  label: datasetFieldLabel(dataset, valueField),
                  format: dataset.measures.find(
                    (measure) => measure.id === valueField
                  )?.format,
                },
              })
            }
          >
            <SelectTrigger>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {outputFields.map((field) => (
                <SelectItem key={field} value={field}>
                  {datasetFieldLabel(dataset, field)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Field>
      )}
    </div>
  );
}

function SourceJoinsEditor({
  block,
  schemas,
  baseFields,
  onChange,
}: {
  block: ReportBlockDefinition;
  schemas: Schema[];
  baseFields: string[];
  onChange: (block: ReportBlockDefinition) => void;
}) {
  const joins = block.source.join ?? [];

  const updateJoins = (nextJoins: ReportSourceJoin[]) => {
    onChange({
      ...block,
      source: {
        ...block.source,
        join: nextJoins,
      },
    });
  };

  const updateJoin = (index: number, patch: Partial<ReportSourceJoin>) => {
    updateJoins(
      joins.map((join, currentIndex) =>
        currentIndex === index ? { ...join, ...patch } : join
      )
    );
  };

  const addJoin = () => {
    const schema =
      schemas.find((candidate) => candidate.name !== block.source.schema) ??
      schemas[0];
    const joinedFields = getSchemaFields(schema);
    updateJoins([
      ...joins,
      {
        schema: schema?.name ?? '',
        alias: uniqueJoinAlias(joins, schema?.name ?? 'joined'),
        parentField: baseFields[0] ?? 'id',
        field: joinedFields[0] ?? 'id',
        op: 'eq',
        kind: 'left',
      },
    ]);
  };

  return (
    <div className="flex flex-col gap-3 rounded-md border bg-muted/10 p-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          <Label>Source joins</Label>
          <Badge variant="secondary">{joins.length} joins</Badge>
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={schemas.length === 0 || baseFields.length === 0}
          onClick={addJoin}
        >
          <Plus className="mr-2 size-4" />
          Add join
        </Button>
      </div>
      {joins.length === 0 ? (
        <div className="rounded-md border border-dashed bg-background p-3 text-sm text-muted-foreground">
          No joins. Add one to expose joined fields as alias.field in columns,
          groupings, aggregates, filters, and cards.
        </div>
      ) : (
        <div className="flex flex-col gap-2">
          {joins.map((join, index) => {
            const joinedSchema = schemas.find(
              (candidate) => candidate.name === join.schema
            );
            const joinedFields = getSchemaFields(joinedSchema);
            return (
              <div
                key={`join-${index}-${join.alias ?? join.schema}`}
                className="grid gap-2 rounded-md border bg-background p-3 md:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_minmax(0,1fr)_minmax(0,1fr)_9rem_9rem_40px]"
              >
                <Field label="Joined schema">
                  <Select
                    value={join.schema || NONE_VALUE}
                    onValueChange={(schemaName) => {
                      const nextSchema = schemas.find(
                        (candidate) => candidate.name === schemaName
                      );
                      const nextFields = getSchemaFields(nextSchema);
                      updateJoin(index, {
                        schema: schemaName,
                        alias: uniqueJoinAlias(joins, schemaName, index),
                        field: nextFields.includes(join.field)
                          ? join.field
                          : (nextFields[0] ?? join.field),
                      });
                    }}
                  >
                    <SelectTrigger>
                      <SelectValue placeholder="Schema" />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value={NONE_VALUE} disabled>
                        Select schema
                      </SelectItem>
                      {schemas.map((schema) => (
                        <SelectItem key={schema.id} value={schema.name}>
                          {schema.name}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </Field>
                <Field label="Alias">
                  <Input
                    value={join.alias ?? ''}
                    placeholder={slugify(join.schema).replace(/-/g, '_')}
                    onChange={(event) =>
                      updateJoin(index, {
                        alias:
                          slugify(event.target.value).replace(/-/g, '_') ||
                          undefined,
                      })
                    }
                  />
                </Field>
                <Field label="Parent field">
                  <Select
                    value={join.parentField || NONE_VALUE}
                    onValueChange={(parentField) =>
                      updateJoin(index, { parentField })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value={NONE_VALUE} disabled>
                        Select field
                      </SelectItem>
                      {baseFields.map((field) => (
                        <SelectItem key={field} value={field}>
                          {field}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </Field>
                <Field label="Joined field">
                  <Select
                    value={join.field || NONE_VALUE}
                    onValueChange={(field) => updateJoin(index, { field })}
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value={NONE_VALUE} disabled>
                        Select field
                      </SelectItem>
                      {joinedFields.map((field) => (
                        <SelectItem key={field} value={field}>
                          {field}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </Field>
                <Field label="Kind">
                  <Select
                    value={join.kind ?? 'inner'}
                    onValueChange={(kind) =>
                      updateJoin(index, {
                        kind: kind as NonNullable<ReportSourceJoin['kind']>,
                      })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {JOIN_KIND_OPTIONS.map((option) => (
                        <SelectItem key={option.value} value={option.value}>
                          {option.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </Field>
                <Field label="Operator">
                  <Select
                    value={join.op ?? 'eq'}
                    onValueChange={(op) => updateJoin(index, { op })}
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {CONDITION_OPERATOR_OPTIONS.map((option) => (
                        <SelectItem key={option.value} value={option.value}>
                          {option.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </Field>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="mt-6 size-9"
                  onClick={() =>
                    updateJoins(
                      joins.filter((_, currentIndex) => currentIndex !== index)
                    )
                  }
                  aria-label="Remove join"
                >
                  <Trash2 className="size-4" />
                </Button>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

function TableBlockSettings({
  block,
  fields,
  onChange,
}: {
  block: ReportBlockDefinition;
  fields: string[];
  onChange: (block: ReportBlockDefinition) => void;
}) {
  const columns = block.table?.columns ?? [];
  const selectedFields = new Set(columns.map((column) => column.field));
  const defaultSort = block.table?.defaultSort?.[0];

  const updateColumns = (nextColumns: ReportTableColumn[]) => {
    onChange({
      ...block,
      table: {
        ...block.table,
        columns: nextColumns,
        pagination: block.table?.pagination ?? {
          defaultPageSize: 50,
          allowedPageSizes: [25, 50, 100],
        },
      },
      source: { ...block.source, mode: 'filter' },
    });
  };
  const updateColumn = (index: number, patch: Partial<ReportTableColumn>) => {
    updateColumns(
      columns.map((column, currentIndex) =>
        currentIndex === index ? { ...column, ...patch } : column
      )
    );
  };

  return (
    <div className="flex flex-col gap-4">
      <div className="grid gap-3 md:grid-cols-[minmax(0,1fr)_220px]">
        <div className="flex flex-col gap-2">
          <Label>Columns</Label>
          <div className="grid gap-2 sm:grid-cols-2 xl:grid-cols-3">
            {fields.map((field) => (
              <label
                key={field}
                className="flex min-h-10 items-center gap-2 rounded-md border px-3 py-2 text-sm"
              >
                <Checkbox
                  checked={selectedFields.has(field)}
                  onCheckedChange={(checked) => {
                    const nextColumns = checked
                      ? [...columns, { field, label: humanizeFieldName(field) }]
                      : columns.filter((column) => column.field !== field);
                    updateColumns(nextColumns);
                  }}
                />
                <span className="truncate">{field}</span>
              </label>
            ))}
          </div>
        </div>
        <div className="flex flex-col gap-3">
          <Field label="Page size">
            <Input
              type="number"
              min={1}
              value={block.table?.pagination?.defaultPageSize ?? 50}
              onChange={(event) =>
                onChange({
                  ...block,
                  table: {
                    ...block.table,
                    pagination: {
                      defaultPageSize: Number(event.target.value) || 50,
                      allowedPageSizes: block.table?.pagination
                        ?.allowedPageSizes ?? [25, 50, 100],
                    },
                  },
                })
              }
            />
          </Field>
          <Field label="Sort field">
            <Select
              value={defaultSort?.field ?? NONE_VALUE}
              onValueChange={(field) =>
                onChange({
                  ...block,
                  table: {
                    ...block.table,
                    defaultSort:
                      field === NONE_VALUE
                        ? []
                        : [
                            {
                              field,
                              direction: defaultSort?.direction ?? 'asc',
                            },
                          ],
                  },
                })
              }
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={NONE_VALUE}>None</SelectItem>
                {fields.map((field) => (
                  <SelectItem key={field} value={field}>
                    {field}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </Field>
          <Field label="Sort direction">
            <Select
              value={defaultSort?.direction ?? 'asc'}
              disabled={!defaultSort?.field}
              onValueChange={(direction) =>
                onChange({
                  ...block,
                  table: {
                    ...block.table,
                    defaultSort: defaultSort?.field
                      ? [{ field: defaultSort.field, direction }]
                      : [],
                  },
                })
              }
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="asc">Ascending</SelectItem>
                <SelectItem value="desc">Descending</SelectItem>
              </SelectContent>
            </Select>
          </Field>
        </div>
      </div>
      {columns.length > 0 && (
        <div className="flex flex-col gap-3">
          {columns.map((column, index) => (
            <div key={column.field} className="rounded-md border p-3">
              <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
                <div className="min-w-0">
                  <div className="truncate text-sm font-semibold text-foreground">
                    {column.label || humanizeFieldName(column.field)}
                  </div>
                  <div className="text-xs text-muted-foreground">
                    {column.field}
                  </div>
                </div>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="size-8"
                  onClick={() =>
                    updateColumns(
                      columns.filter(
                        (_, currentIndex) => currentIndex !== index
                      )
                    )
                  }
                  aria-label={`Remove ${column.field}`}
                >
                  <Trash2 className="size-4" />
                </Button>
              </div>
              <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-4">
                <Field label="Label">
                  <Input
                    value={column.label ?? ''}
                    placeholder={humanizeFieldName(column.field)}
                    onChange={(event) =>
                      updateColumn(index, { label: event.target.value })
                    }
                  />
                </Field>
                <Field label="Format">
                  <Select
                    value={column.format ?? NONE_VALUE}
                    onValueChange={(format) =>
                      updateColumn(index, {
                        format: format === NONE_VALUE ? undefined : format,
                      })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {COLUMN_FORMAT_OPTIONS.map((option) => (
                        <SelectItem key={option.value} value={option.value}>
                          {option.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </Field>
                <Field label="Align">
                  <Select
                    value={column.align ?? 'left'}
                    onValueChange={(align) =>
                      updateColumn(index, {
                        align: align as ReportTableColumn['align'],
                      })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {ALIGN_OPTIONS.map((option) => (
                        <SelectItem key={option.value} value={option.value}>
                          {option.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </Field>
                <Field label="Display field">
                  <Select
                    value={column.displayField ?? NONE_VALUE}
                    onValueChange={(field) =>
                      updateColumn(index, {
                        displayField: field === NONE_VALUE ? undefined : field,
                      })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value={NONE_VALUE}>Same as field</SelectItem>
                      {fields.map((field) => (
                        <SelectItem key={field} value={field}>
                          {field}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </Field>
                <Field label="Secondary field">
                  <Select
                    value={column.secondaryField ?? NONE_VALUE}
                    onValueChange={(field) =>
                      updateColumn(index, {
                        secondaryField:
                          field === NONE_VALUE ? undefined : field,
                      })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value={NONE_VALUE}>None</SelectItem>
                      {fields.map((field) => (
                        <SelectItem key={field} value={field}>
                          {field}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </Field>
                <Field label="Link field">
                  <Select
                    value={column.linkField ?? NONE_VALUE}
                    onValueChange={(field) =>
                      updateColumn(index, {
                        linkField: field === NONE_VALUE ? undefined : field,
                      })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value={NONE_VALUE}>None</SelectItem>
                      {fields.map((field) => (
                        <SelectItem key={field} value={field}>
                          {field}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </Field>
                <Field label="Tooltip field">
                  <Select
                    value={column.tooltipField ?? NONE_VALUE}
                    onValueChange={(field) =>
                      updateColumn(index, {
                        tooltipField: field === NONE_VALUE ? undefined : field,
                      })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value={NONE_VALUE}>None</SelectItem>
                      {fields.map((field) => (
                        <SelectItem key={field} value={field}>
                          {field}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </Field>
                <Field label="Pill variants">
                  <Input
                    value={formatPillVariants(column.pillVariants)}
                    placeholder="open:success, closed:muted"
                    disabled={column.format !== 'pill'}
                    onChange={(event) =>
                      updateColumn(index, {
                        pillVariants: parsePillVariants(event.target.value),
                      })
                    }
                  />
                </Field>
              </div>
              <div className="mt-3 grid gap-2 sm:grid-cols-2 xl:grid-cols-3">
                <label className="flex min-h-10 items-center gap-2 rounded-md border px-3 py-2 text-sm">
                  <Checkbox
                    checked={Boolean(column.editable)}
                    onCheckedChange={(checked) =>
                      updateColumn(index, { editable: Boolean(checked) })
                    }
                  />
                  Editable
                </label>
                <label className="flex min-h-10 items-center gap-2 rounded-md border px-3 py-2 text-sm">
                  <Checkbox
                    checked={Boolean(column.descriptive)}
                    onCheckedChange={(checked) =>
                      updateColumn(index, {
                        descriptive: Boolean(checked),
                      })
                    }
                  />
                  Descriptive label
                </label>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function MetricBlockSettings({
  block,
  fields,
  onChange,
}: {
  block: ReportBlockDefinition;
  fields: string[];
  onChange: (block: ReportBlockDefinition) => void;
}) {
  const aggregate = block.source.aggregates?.[0] ?? {
    alias: 'value',
    op: 'count' as ReportAggregateFn,
  };

  const updateAggregate = (
    patch: Partial<
      NonNullable<ReportBlockDefinition['source']['aggregates']>[number]
    >
  ) => {
    const nextAggregate = { ...aggregate, ...patch, alias: 'value' };
    onChange({
      ...block,
      source: {
        ...block.source,
        mode: 'aggregate',
        groupBy: [],
        aggregates: [nextAggregate],
      },
      metric: {
        valueField: 'value',
        label: block.metric?.label ?? block.title ?? 'Metric',
        format: block.metric?.format ?? 'number',
      },
    });
  };

  return (
    <div className="grid gap-3 md:grid-cols-4">
      <Field label="Aggregate">
        <Select
          value={aggregate.op}
          onValueChange={(op) =>
            updateAggregate({
              op: op as ReportAggregateFn,
              field: op === 'count' ? undefined : aggregate.field || fields[0],
            })
          }
        >
          <SelectTrigger>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {AGGREGATE_OPTIONS.map((option) => (
              <SelectItem key={option.value} value={option.value}>
                {option.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </Field>
      <Field label="Field">
        <Select
          value={aggregate.field ?? NONE_VALUE}
          disabled={aggregate.op === 'count'}
          onValueChange={(field) =>
            updateAggregate({ field: field === NONE_VALUE ? undefined : field })
          }
        >
          <SelectTrigger>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={NONE_VALUE}>Any record</SelectItem>
            {fields.map((field) => (
              <SelectItem key={field} value={field}>
                {field}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </Field>
      <Field label="Metric label">
        <Input
          value={block.metric?.label ?? ''}
          placeholder={block.title ?? 'Metric'}
          onChange={(event) =>
            onChange({
              ...block,
              metric: {
                valueField: 'value',
                label: event.target.value,
                format: block.metric?.format ?? 'number',
              },
            })
          }
        />
      </Field>
      <Field label="Format">
        <Select
          value={block.metric?.format ?? 'number'}
          onValueChange={(format) =>
            onChange({
              ...block,
              metric: {
                valueField: 'value',
                label: block.metric?.label,
                format,
              },
            })
          }
        >
          <SelectTrigger>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="number">Number</SelectItem>
            <SelectItem value="currency">Currency</SelectItem>
            <SelectItem value="percent">Percent</SelectItem>
          </SelectContent>
        </Select>
      </Field>
    </div>
  );
}

function ChartBlockSettings({
  block,
  fields,
  onChange,
}: {
  block: ReportBlockDefinition;
  fields: string[];
  onChange: (block: ReportBlockDefinition) => void;
}) {
  const xField =
    block.chart?.x ?? block.source.groupBy?.[0] ?? fields[0] ?? 'id';
  const aggregate = block.source.aggregates?.[0] ?? {
    alias: 'value',
    op: 'count' as ReportAggregateFn,
  };
  const series = block.chart?.series?.[0] ?? {
    field: aggregate.alias,
    label: humanizeFieldName(aggregate.alias),
  };

  const updateChart = ({
    kind = block.chart?.kind ?? 'bar',
    x = xField,
    aggregatePatch = {},
    seriesLabel = series.label,
    limit = block.source.limit ?? 100,
  }: {
    kind?: ReportChartKind;
    x?: string;
    aggregatePatch?: Partial<typeof aggregate>;
    seriesLabel?: string;
    limit?: number;
  }) => {
    const nextAggregate = {
      ...aggregate,
      ...aggregatePatch,
      alias: aggregate.alias || 'value',
    };
    onChange({
      ...block,
      source: {
        ...block.source,
        mode: 'aggregate',
        groupBy: [x],
        aggregates: [nextAggregate],
        limit,
      },
      chart: {
        kind,
        x,
        series: [
          {
            field: nextAggregate.alias,
            label: seriesLabel,
          },
        ],
      },
    });
  };

  return (
    <div className="grid gap-3 md:grid-cols-3 xl:grid-cols-6">
      <Field label="Chart">
        <Select
          value={block.chart?.kind ?? 'bar'}
          onValueChange={(kind) =>
            updateChart({ kind: kind as ReportChartKind })
          }
        >
          <SelectTrigger>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {CHART_KIND_OPTIONS.map((option) => (
              <SelectItem key={option.value} value={option.value}>
                {option.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </Field>
      <Field label="X field">
        <Select value={xField} onValueChange={(x) => updateChart({ x })}>
          <SelectTrigger>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {fields.map((field) => (
              <SelectItem key={field} value={field}>
                {field}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </Field>
      <Field label="Aggregate">
        <Select
          value={aggregate.op}
          onValueChange={(op) =>
            updateChart({
              aggregatePatch: {
                op: op as ReportAggregateFn,
                field:
                  op === 'count' ? undefined : aggregate.field || fields[0],
              },
            })
          }
        >
          <SelectTrigger>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {AGGREGATE_OPTIONS.map((option) => (
              <SelectItem key={option.value} value={option.value}>
                {option.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </Field>
      <Field label="Value field">
        <Select
          value={aggregate.field ?? NONE_VALUE}
          disabled={aggregate.op === 'count'}
          onValueChange={(field) =>
            updateChart({
              aggregatePatch: {
                field: field === NONE_VALUE ? undefined : field,
              },
            })
          }
        >
          <SelectTrigger>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={NONE_VALUE}>Any record</SelectItem>
            {fields.map((field) => (
              <SelectItem key={field} value={field}>
                {field}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </Field>
      <Field label="Series label">
        <Input
          value={series.label ?? ''}
          placeholder="Value"
          onChange={(event) => updateChart({ seriesLabel: event.target.value })}
        />
      </Field>
      <Field label="Limit">
        <Input
          type="number"
          min={1}
          value={block.source.limit ?? 100}
          onChange={(event) =>
            updateChart({ limit: Number(event.target.value) || 100 })
          }
        />
      </Field>
    </div>
  );
}

function CardBlockSettings({
  block,
  fields,
  onChange,
}: {
  block: ReportBlockDefinition;
  fields: string[];
  onChange: (block: ReportBlockDefinition) => void;
}) {
  const groups = block.card?.groups ?? [];
  const primaryGroup = groups[0] ?? {
    id: 'details',
    title: 'Details',
    columns: 2,
    fields: [],
  };
  const selectedFields = new Set(
    primaryGroup.fields.map((field) => field.field)
  );

  const updateGroup = (nextGroup: typeof primaryGroup) => {
    onChange({
      ...block,
      source: { ...block.source, mode: 'filter', limit: 1 },
      card: {
        groups: [nextGroup, ...groups.slice(1)],
      },
    });
  };

  const updateField = (
    index: number,
    patch: Partial<(typeof primaryGroup.fields)[number]>
  ) => {
    updateGroup({
      ...primaryGroup,
      fields: primaryGroup.fields.map((field, currentIndex) =>
        currentIndex === index ? { ...field, ...patch } : field
      ),
    });
  };

  return (
    <div className="flex flex-col gap-4">
      <div className="grid gap-3 md:grid-cols-3">
        <Field label="Group title">
          <Input
            value={primaryGroup.title ?? ''}
            placeholder="Details"
            onChange={(event) =>
              updateGroup({ ...primaryGroup, title: event.target.value })
            }
          />
        </Field>
        <Field label="Columns">
          <Input
            type="number"
            min={1}
            max={4}
            value={primaryGroup.columns ?? 2}
            onChange={(event) =>
              updateGroup({
                ...primaryGroup,
                columns: Math.min(
                  4,
                  Math.max(1, Number(event.target.value) || 2)
                ),
              })
            }
          />
        </Field>
        <Field label="Record limit">
          <Input value="1" readOnly className="bg-muted/40" />
        </Field>
      </div>

      <div className="flex flex-col gap-2">
        <Label>Fields</Label>
        <div className="grid gap-2 sm:grid-cols-2 xl:grid-cols-3">
          {fields.map((field) => (
            <label
              key={field}
              className="flex min-h-10 items-center gap-2 rounded-md border px-3 py-2 text-sm"
            >
              <Checkbox
                checked={selectedFields.has(field)}
                onCheckedChange={(checked) => {
                  const nextFields = checked
                    ? [
                        ...primaryGroup.fields,
                        {
                          field,
                          label: humanizeFieldName(field),
                          kind: 'value' as const,
                        },
                      ]
                    : primaryGroup.fields.filter(
                        (item) => item.field !== field
                      );
                  updateGroup({ ...primaryGroup, fields: nextFields });
                }}
              />
              <span className="truncate">{field}</span>
            </label>
          ))}
        </div>
      </div>

      {primaryGroup.fields.length > 0 && (
        <div className="flex flex-col gap-3">
          {primaryGroup.fields.map((field, index) => (
            <div key={field.field} className="rounded-md border p-3">
              <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
                <div>
                  <div className="text-sm font-semibold text-foreground">
                    {field.label || humanizeFieldName(field.field)}
                  </div>
                  <div className="text-xs text-muted-foreground">
                    {field.field}
                  </div>
                </div>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="size-8"
                  onClick={() =>
                    updateGroup({
                      ...primaryGroup,
                      fields: primaryGroup.fields.filter(
                        (_, currentIndex) => currentIndex !== index
                      ),
                    })
                  }
                  aria-label={`Remove ${field.field}`}
                >
                  <Trash2 className="size-4" />
                </Button>
              </div>
              <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-4">
                <Field label="Label">
                  <Input
                    value={field.label ?? ''}
                    placeholder={humanizeFieldName(field.field)}
                    onChange={(event) =>
                      updateField(index, { label: event.target.value })
                    }
                  />
                </Field>
                <Field label="Kind">
                  <Select
                    value={field.kind ?? 'value'}
                    onValueChange={(kind) =>
                      updateField(index, {
                        kind: kind as NonNullable<typeof field.kind>,
                      })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="value">Value</SelectItem>
                      <SelectItem value="json">JSON</SelectItem>
                      <SelectItem value="markdown">Markdown</SelectItem>
                    </SelectContent>
                  </Select>
                </Field>
                <Field label="Format">
                  <Select
                    value={field.format ?? NONE_VALUE}
                    onValueChange={(format) =>
                      updateField(index, {
                        format: format === NONE_VALUE ? undefined : format,
                      })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {COLUMN_FORMAT_OPTIONS.map((option) => (
                        <SelectItem key={option.value} value={option.value}>
                          {option.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </Field>
                <Field label="Display field">
                  <Select
                    value={field.displayField ?? NONE_VALUE}
                    onValueChange={(displayField) =>
                      updateField(index, {
                        displayField:
                          displayField === NONE_VALUE
                            ? undefined
                            : displayField,
                      })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value={NONE_VALUE}>Same as field</SelectItem>
                      {fields.map((candidate) => (
                        <SelectItem key={candidate} value={candidate}>
                          {candidate}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </Field>
              </div>
              <div className="mt-3 grid gap-2 sm:grid-cols-2">
                <label className="flex min-h-10 items-center gap-2 rounded-md border px-3 py-2 text-sm">
                  <Checkbox
                    checked={Boolean(field.editable)}
                    disabled={(field.kind ?? 'value') !== 'value'}
                    onCheckedChange={(checked) =>
                      updateField(index, { editable: Boolean(checked) })
                    }
                  />
                  Editable
                </label>
                <label className="flex min-h-10 items-center gap-2 rounded-md border px-3 py-2 text-sm">
                  <Checkbox
                    checked={Boolean(field.collapsed)}
                    onCheckedChange={(checked) =>
                      updateField(index, { collapsed: Boolean(checked) })
                    }
                  />
                  Collapsed by default
                </label>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function BlockFiltersEditor({
  block,
  fields,
  onChange,
}: {
  block: ReportBlockDefinition;
  fields: string[];
  onChange: (block: ReportBlockDefinition) => void;
}) {
  const filters = block.filters ?? [];

  const updateFilter = (index: number, filter: ReportFilterDefinition) => {
    onChange({
      ...block,
      filters: filters.map((current, currentIndex) =>
        currentIndex === index ? filter : current
      ),
    });
  };

  const addFilter = () => {
    const field = fields[0] ?? 'id';
    const id = uniqueFilterId(filters, field);
    onChange({
      ...block,
      filters: [
        ...filters,
        {
          id,
          label: humanizeFieldName(field),
          type: 'select',
          appliesTo: [{ blockId: block.id, field, op: 'eq' }],
          options: { source: 'static', values: [] },
        },
      ],
    });
  };

  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center justify-between gap-3">
        <Label>Block filters</Label>
        <Button type="button" variant="outline" size="sm" onClick={addFilter}>
          <Plus className="mr-2 size-4" />
          Add filter
        </Button>
      </div>
      {filters.length === 0 ? (
        <div className="rounded-md border border-dashed bg-muted/20 p-4" />
      ) : (
        <div className="flex flex-col gap-2">
          {filters.map((filter, index) => {
            const target = filter.appliesTo?.[0];
            return (
              <div
                key={filter.id}
                className="grid gap-2 rounded-md border p-3 md:grid-cols-[1fr_160px_160px_1fr_40px]"
              >
                <Field label="Label">
                  <Input
                    value={filter.label}
                    onChange={(event) =>
                      updateFilter(index, {
                        ...filter,
                        label: event.target.value,
                      })
                    }
                  />
                </Field>
                <Field label="Type">
                  <Select
                    value={filter.type}
                    onValueChange={(type) =>
                      updateFilter(index, {
                        ...filter,
                        type: type as ReportFilterType,
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
                </Field>
                <Field label="Field">
                  <Select
                    value={target?.field ?? fields[0] ?? 'id'}
                    onValueChange={(field) =>
                      updateFilter(index, {
                        ...filter,
                        appliesTo: [
                          {
                            blockId: block.id,
                            field,
                            op: target?.op ?? 'eq',
                          },
                        ],
                      })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {fields.map((field) => (
                        <SelectItem key={field} value={field}>
                          {field}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </Field>
                <Field label="Options">
                  <Input
                    value={formatFilterOptions(filter)}
                    placeholder="open, closed, pending"
                    disabled={!usesStaticOptions(filter.type)}
                    onChange={(event) =>
                      updateFilter(index, {
                        ...filter,
                        options: {
                          source: 'static',
                          values: parseFilterOptions(event.target.value),
                        },
                      })
                    }
                  />
                </Field>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="mt-6 size-9"
                  onClick={() =>
                    onChange({
                      ...block,
                      filters: filters.filter(
                        (_, currentIndex) => currentIndex !== index
                      ),
                    })
                  }
                  aria-label="Remove filter"
                >
                  <Trash2 className="size-4" />
                </Button>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5">
      <Label className="text-xs font-medium text-muted-foreground">
        {label}
      </Label>
      {children}
    </div>
  );
}

function blockToEditorNode(
  block: ReportBlockDefinition,
  layoutId: string
): EditorNode {
  if (block.type === 'markdown') {
    return {
      kind: 'markdown',
      nodeId: `block-${block.id}`,
      layoutId,
      blockId: block.id,
      content: block.markdown?.content ?? '',
    };
  }
  return {
    kind: 'block',
    nodeId: `block-${block.id}`,
    layoutId,
    blockId: block.id,
  };
}

function definitionToNodes(definition: ReportDefinition): EditorNode[] {
  const blockById = new Map(
    definition.blocks.map((block) => [block.id, block])
  );
  if ((definition.layout?.length ?? 0) > 0) {
    const referencedBlockIds = new Set(
      extractLayoutBlockReferences(definition.layout)
    );
    const nodes = (definition.layout ?? []).map((node) =>
      layoutNodeToEditorNode(node, blockById)
    );
    for (const block of definition.blocks) {
      if (!referencedBlockIds.has(block.id)) {
        nodes.push(blockToEditorNode(block, `${block.id}_node`));
      }
    }
    return nodes;
  }

  return definition.blocks.map((block) =>
    blockToEditorNode(block, `${block.id}_node`)
  );
}

function nodesToDefinition(
  definition: ReportDefinition,
  nodes: EditorNode[],
  blocks: ReportBlockDefinition[]
): ReportDefinition {
  const blockById = new Map(blocks.map((block) => [block.id, block]));
  const orderedBlocks: ReportBlockDefinition[] = [];
  const seenBlockIds = new Set<string>();

  for (const node of nodes) {
    if (node.kind === 'markdown') {
      orderedBlocks.push(
        createMarkdownBlock(
          node.blockId,
          node.content,
          blockById.get(node.blockId)
        )
      );
      seenBlockIds.add(node.blockId);
      continue;
    }
    for (const blockId of collectEditorNodeBlockIds([node])) {
      if (seenBlockIds.has(blockId)) continue;
      const block = blockById.get(blockId);
      if (!block) continue;
      orderedBlocks.push(block);
      seenBlockIds.add(blockId);
    }
  }

  for (const block of blocks) {
    if (seenBlockIds.has(block.id)) continue;
    if (block.type !== 'markdown') {
      orderedBlocks.push(block);
    }
  }

  return {
    ...definition,
    layout: nodes.map(editorNodeToLayoutNode),
    blocks: orderedBlocks,
  };
}

function layoutNodeToEditorNode(
  node: ReportLayoutNode,
  blockById: Map<string, ReportBlockDefinition>
): EditorNode {
  if (node.type === 'block') {
    const block = blockById.get(node.blockId);
    if (block?.type === 'markdown') {
      return blockToEditorNode(block, node.id);
    }
    return {
      kind: 'block',
      nodeId: `layout-${node.id}`,
      layoutId: node.id,
      blockId: node.blockId,
    };
  }
  if (node.type === 'metric_row') {
    return {
      kind: 'metric_row',
      nodeId: `layout-${node.id}`,
      layoutId: node.id,
      title: node.title,
      blocks: node.blocks,
    };
  }
  return {
    kind: 'layout',
    nodeId: `layout-${node.id}`,
    layout: node,
  };
}

function editorNodeToLayoutNode(node: EditorNode): ReportLayoutNode {
  if (node.kind === 'markdown') {
    return {
      id: node.layoutId ?? node.nodeId,
      type: 'block',
      blockId: node.blockId,
    };
  }
  if (node.kind === 'block') {
    return {
      id: node.layoutId ?? `${node.blockId}_node`,
      type: 'block',
      blockId: node.blockId,
    };
  }
  if (node.kind === 'metric_row') {
    return {
      id: node.layoutId ?? node.nodeId,
      type: 'metric_row',
      title: node.title || undefined,
      blocks: node.blocks,
    };
  }
  return node.layout;
}

function collectEditorNodeBlockIds(nodes: EditorNode[]): string[] {
  return nodes.flatMap((node) => {
    if (node.kind === 'markdown') return [node.blockId];
    if (node.kind === 'block') return [node.blockId];
    if (node.kind === 'metric_row') return node.blocks;
    if (node.kind === 'layout')
      return extractLayoutBlockReferences([node.layout]);
    return [];
  });
}

function replaceBlockIdInNodes(
  nodes: EditorNode[],
  previousBlockId: string,
  nextBlockId: string
): EditorNode[] {
  return nodes.map((node) => {
    if (
      (node.kind === 'block' || node.kind === 'markdown') &&
      node.blockId === previousBlockId
    ) {
      return { ...node, blockId: nextBlockId };
    }
    if (node.kind === 'metric_row') {
      return {
        ...node,
        blocks: node.blocks.map((blockId) =>
          blockId === previousBlockId ? nextBlockId : blockId
        ),
      };
    }
    if (node.kind === 'layout') {
      return {
        ...node,
        layout: replaceBlockIdInLayoutNode(
          node.layout,
          previousBlockId,
          nextBlockId
        ),
      };
    }
    return node;
  });
}

function replaceBlockIdInLayoutNode(
  node: ReportLayoutNode,
  previousBlockId: string,
  nextBlockId: string
): ReportLayoutNode {
  if (node.type === 'block') {
    return {
      ...node,
      blockId: node.blockId === previousBlockId ? nextBlockId : node.blockId,
    };
  }
  if (node.type === 'metric_row') {
    return {
      ...node,
      blocks: node.blocks.map((blockId) =>
        blockId === previousBlockId ? nextBlockId : blockId
      ),
    };
  }
  if (node.type === 'section') {
    return {
      ...node,
      children: node.children?.map((child) =>
        replaceBlockIdInLayoutNode(child, previousBlockId, nextBlockId)
      ),
    };
  }
  if (node.type === 'columns') {
    return {
      ...node,
      columns: node.columns.map((column) => ({
        ...column,
        children: column.children?.map((child) =>
          replaceBlockIdInLayoutNode(child, previousBlockId, nextBlockId)
        ),
      })),
    };
  }
  if (node.type === 'grid') {
    return {
      ...node,
      items: node.items.map((item) => ({
        ...item,
        blockId: item.blockId === previousBlockId ? nextBlockId : item.blockId,
      })),
    };
  }
  return node;
}

function insertAfter<T>(items: T[], index: number, item: T): T[] {
  if (index < 0) return [...items, item];
  return [...items.slice(0, index + 1), item, ...items.slice(index + 1)];
}

function moveItem<T>(items: T[], fromIndex: number, toIndex: number): T[] {
  const nextItems = [...items];
  const [item] = nextItems.splice(fromIndex, 1);
  nextItems.splice(toIndex, 0, item);
  return nextItems;
}

function getSchemaFields(schema: Schema | undefined): string[] {
  const schemaFields = schema?.columns?.map((column) => column.name) ?? [];
  return ['id', ...schemaFields, 'createdAt', 'updatedAt'];
}

function datasetFilterableFields(dataset: ReportDatasetDefinition): string[] {
  return Array.from(
    new Set([
      ...dataset.dimensions.map((dimension) => dimension.field),
      ...(dataset.timeDimension ? [dataset.timeDimension] : []),
      ...dataset.measures.flatMap((measure) =>
        measure.field ? [measure.field] : []
      ),
    ])
  ).filter(Boolean);
}

function formatDatasetFilterValue(value: unknown, op?: string): string {
  if (op === 'between' && isRecord(value)) {
    const from = value.min ?? value.from ?? '';
    const to = value.max ?? value.to ?? '';
    return `${from}..${to}`;
  }
  if (Array.isArray(value)) return value.map(String).join(', ');
  if (value === null || value === undefined) return '';
  return String(value);
}

function parseDatasetFilterValue(value: string, op?: string): unknown {
  const trimmed = value.trim();
  if (op === 'between') {
    const [min = '', max = ''] = trimmed.includes('..')
      ? trimmed.split('..')
      : trimmed.split(',');
    return {
      min: parseDatasetScalar(min.trim()),
      max: parseDatasetScalar(max.trim()),
    };
  }
  if (op === 'in') {
    return trimmed
      .split(',')
      .map((part) => parseDatasetScalar(part.trim()))
      .filter((part) => part !== '');
  }
  return parseDatasetScalar(trimmed);
}

function parseDatasetScalar(value: string): unknown {
  if (value === '') return '';
  if (value === 'true') return true;
  if (value === 'false') return false;
  if (/^-?\d+(\.\d+)?$/.test(value)) return Number(value);
  return value;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function getBlockAvailableFields(
  schema: Schema | undefined,
  joins: ReportSourceJoin[] | undefined,
  schemas: Schema[]
): string[] {
  const baseFields = getSchemaFields(schema);
  const joinedFields =
    joins?.flatMap((join) => {
      const alias = effectiveJoinAlias(join);
      const joinedSchema = schemas.find(
        (candidate) => candidate.name === join.schema
      );
      if (!alias || !joinedSchema) return [];
      return getSchemaFields(joinedSchema).map((field) => `${alias}.${field}`);
    }) ?? [];
  return Array.from(new Set([...baseFields, ...joinedFields]));
}

function effectiveJoinAlias(join: ReportSourceJoin): string {
  return join.alias?.trim() || join.schema;
}

function uniqueJoinAlias(
  joins: ReportSourceJoin[],
  seed: string,
  ignoreIndex?: number
): string {
  const existing = new Set(
    joins
      .filter((_, index) => index !== ignoreIndex)
      .map(effectiveJoinAlias)
      .filter(Boolean)
  );
  const base = slugify(seed || 'joined').replace(/-/g, '_') || 'joined';
  let candidate = base;
  let suffix = 2;
  while (existing.has(candidate)) {
    candidate = `${base}_${suffix}`;
    suffix += 1;
  }
  return candidate;
}

function emptyReportSource(): ReportBlockDefinition['source'] {
  return {
    schema: '',
    mode: 'filter',
    groupBy: [],
    aggregates: [],
    orderBy: [],
    filterMappings: [],
    join: [],
  };
}

function createMarkdownBlock(
  id: string,
  content: string,
  existing?: ReportBlockDefinition
): ReportBlockDefinition {
  return {
    id,
    type: 'markdown',
    title: existing?.title,
    lazy: existing?.lazy ?? false,
    source: existing?.source ?? emptyReportSource(),
    dataset: existing?.dataset,
    markdown: { content: content.trim() },
    filters: existing?.filters ?? [],
    interactions: existing?.interactions ?? [],
    showWhen: existing?.showWhen,
    hideWhenEmpty: existing?.hideWhenEmpty,
  };
}

function createDefaultBlock(
  type: Exclude<ReportBlockType, 'markdown'>,
  schemaName: string,
  existingBlocks: ReportBlockDefinition[]
): ReportBlockDefinition {
  const id = uniqueBlockId(existingBlocks, type);
  const title = humanizeFieldName(id);
  const source = { schema: schemaName, mode: 'filter' as const };

  if (type === 'metric') {
    return {
      id,
      type,
      title,
      lazy: false,
      source: {
        ...source,
        mode: 'aggregate',
        aggregates: [{ alias: 'value', op: 'count' }],
      },
      metric: {
        valueField: 'value',
        label: title,
        format: 'number',
      },
      filters: [],
    };
  }

  if (type === 'chart') {
    return {
      id,
      type,
      title,
      lazy: false,
      source: {
        ...source,
        mode: 'aggregate',
        groupBy: ['id'],
        aggregates: [{ alias: 'value', op: 'count' }],
        limit: 100,
      },
      chart: {
        kind: 'bar',
        x: 'id',
        series: [{ field: 'value', label: 'Value' }],
      },
      filters: [],
    };
  }

  if (type === 'card') {
    return {
      id,
      type,
      title,
      lazy: false,
      source: { ...source, limit: 1 },
      card: {
        groups: [
          {
            id: 'details',
            title: 'Details',
            columns: 2,
            fields: [],
          },
        ],
      },
      filters: [],
    };
  }

  return {
    id,
    type,
    title,
    lazy: false,
    source,
    table: {
      columns: [],
      pagination: {
        defaultPageSize: 50,
        allowedPageSizes: [25, 50, 100],
      },
    },
    filters: [],
  };
}

function convertBlockType(
  block: ReportBlockDefinition,
  type: Exclude<ReportBlockType, 'markdown'>,
  fields: string[]
): ReportBlockDefinition {
  const base = {
    id: block.id,
    type,
    title: block.title,
    lazy: block.lazy,
    source: {
      ...block.source,
      mode: type === 'table' ? 'filter' : 'aggregate',
    },
    filters: block.filters ?? [],
  };
  const firstField = fields[0] ?? 'id';

  if (type === 'metric') {
    return {
      ...base,
      source: {
        ...base.source,
        mode: 'aggregate',
        groupBy: [],
        aggregates: [{ alias: 'value', op: 'count' }],
      },
      metric: {
        valueField: 'value',
        label: block.metric?.label ?? block.title ?? 'Metric',
        format: block.metric?.format ?? 'number',
      },
    };
  }

  if (type === 'chart') {
    return {
      ...base,
      source: {
        ...base.source,
        mode: 'aggregate',
        groupBy: [firstField],
        aggregates: [{ alias: 'value', op: 'count' }],
        limit: block.source.limit ?? 100,
      },
      chart: {
        kind: block.chart?.kind ?? 'bar',
        x: firstField,
        series: [{ field: 'value', label: 'Value' }],
      },
    };
  }

  if (type === 'card') {
    return {
      ...base,
      source: {
        ...base.source,
        mode: 'filter',
        groupBy: [],
        aggregates: [],
        limit: 1,
      },
      card: block.card ?? {
        groups: [
          {
            id: 'details',
            title: 'Details',
            columns: 2,
            fields: [],
          },
        ],
      },
    };
  }

  return {
    ...base,
    source: {
      ...base.source,
      mode: 'filter',
      groupBy: [],
      aggregates: [],
    },
    table: {
      columns: block.table?.columns ?? [],
      defaultSort: block.table?.defaultSort ?? [],
      pagination: block.table?.pagination ?? {
        defaultPageSize: 50,
        allowedPageSizes: [25, 50, 100],
      },
    },
  };
}

function changeBlockSchema(
  block: ReportBlockDefinition,
  schemaName: string,
  schemas: Schema[]
): ReportBlockDefinition {
  const schema = schemas.find((candidate) => candidate.name === schemaName);
  const fields = getSchemaFields(schema);
  const nextBlock = {
    ...block,
    source: { ...block.source, schema: schemaName },
  };

  if (block.type === 'table') {
    return {
      ...nextBlock,
      table: {
        ...block.table,
        columns: fields.slice(0, 6).map((field) => ({
          field,
          label: humanizeFieldName(field),
        })),
      },
    };
  }

  if (block.type === 'chart') {
    return convertBlockType(nextBlock, 'chart', fields);
  }

  if (block.type === 'metric') {
    return convertBlockType(nextBlock, 'metric', fields);
  }

  if (block.type === 'card') {
    return {
      ...convertBlockType(nextBlock, 'card', fields),
      card: {
        groups: [
          {
            id: 'details',
            title: 'Details',
            columns: 2,
            fields: fields.slice(0, 6).map((field) => ({
              field,
              label: humanizeFieldName(field),
              kind: 'value' as const,
            })),
          },
        ],
      },
    };
  }

  return nextBlock;
}

function duplicateBlockDefinition(
  block: ReportBlockDefinition,
  existingBlocks: ReportBlockDefinition[]
): ReportBlockDefinition {
  const id = uniqueBlockId(existingBlocks, block.id);
  return {
    ...structuredClone(block),
    id,
    title: `${block.title ?? humanizeFieldName(block.id)} copy`,
  };
}

function uniqueBlockId(
  existingBlocks: ReportBlockDefinition[],
  seed: string
): string {
  const existingIds = new Set(existingBlocks.map((block) => block.id));
  const base = slugify(seed || 'block').replace(/-/g, '_') || 'block';
  let candidate = base;
  let suffix = 1;
  while (existingIds.has(candidate)) {
    suffix += 1;
    candidate = `${base}_${suffix}`;
  }
  return candidate;
}

function uniqueLayoutNodeId(layout: ReportLayoutNode[], seed: string): string {
  const existingIds = new Set<string>();
  for (const node of layout) {
    collectLayoutNodeIds(node, existingIds);
  }
  const base =
    slugify(seed || 'layout_node').replace(/-/g, '_') || 'layout_node';
  let candidate = base;
  let suffix = 1;
  while (existingIds.has(candidate)) {
    suffix += 1;
    candidate = `${base}_${suffix}`;
  }
  return candidate;
}

function collectLayoutNodeIds(node: ReportLayoutNode, ids: Set<string>) {
  ids.add(node.id);
  if (node.type === 'section') {
    for (const child of node.children ?? []) collectLayoutNodeIds(child, ids);
    return;
  }
  if (node.type === 'columns') {
    for (const column of node.columns) {
      for (const child of column.children ?? [])
        collectLayoutNodeIds(child, ids);
    }
  }
}

function uniqueFilterId(
  filters: ReportFilterDefinition[],
  seed: string
): string {
  const existingIds = new Set(filters.map((filter) => filter.id));
  const base = slugify(seed || 'filter').replace(/-/g, '_') || 'filter';
  let candidate = `${base}_filter`;
  let suffix = 1;
  while (existingIds.has(candidate)) {
    suffix += 1;
    candidate = `${base}_filter_${suffix}`;
  }
  return candidate;
}

function usesStaticOptions(type: ReportFilterType): boolean {
  return type === 'select' || type === 'radio' || type === 'multi_select';
}

function formatFilterOptions(filter: ReportFilterDefinition): string {
  return (
    filter.options?.values?.map((option) => String(option.value)).join(', ') ??
    ''
  );
}

function parseFilterOptions(value: string) {
  return value
    .split(',')
    .map((part) => part.trim())
    .filter(Boolean)
    .map((part) => ({ label: humanizeFieldName(part), value: part }));
}

function formatPillVariants(variants: Record<string, string> | undefined) {
  if (!variants) return '';
  return Object.entries(variants)
    .map(([value, variant]) => `${value}:${variant}`)
    .join(', ');
}

function parsePillVariants(value: string) {
  const variants = Object.fromEntries(
    value
      .split(',')
      .map((part) => part.trim())
      .filter(Boolean)
      .map((part) => {
        const [key, variant = 'default'] = part.split(':');
        return [key.trim(), variant.trim()];
      })
      .filter(([key]) => key.length > 0)
  );
  return Object.keys(variants).length > 0 ? variants : undefined;
}
