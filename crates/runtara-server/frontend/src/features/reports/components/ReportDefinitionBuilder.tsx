import { DragEvent, ReactNode, useMemo, useState } from 'react';
import {
  Copy,
  GripVertical,
  LineChart,
  Plus,
  Rows3,
  Sigma,
  Text,
  Trash2,
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
  ReportDatasetDefinition,
  ReportDefinition,
  ReportFilterDefinition,
  ReportFilterType,
  ReportLayoutNode,
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

const BLOCK_PLACEHOLDER_RE = /\{\{\s*block\.([a-zA-Z0-9_-]+)\s*\}\}/g;
const NONE_VALUE = '__none__';

const BLOCK_TYPE_META: Record<
  Exclude<ReportBlockType, 'markdown'>,
  { label: string; icon: typeof Rows3 }
> = {
  table: { label: 'Table', icon: Rows3 },
  metric: { label: 'Metric', icon: Sigma },
  chart: { label: 'Chart', icon: LineChart },
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
];

const FILTER_TYPE_OPTIONS: Array<{
  label: string;
  value: ReportFilterType;
}> = [
  { label: 'Select', value: 'select' },
  { label: 'Multi-select', value: 'multi_select' },
  { label: 'Radio', value: 'radio' },
  { label: 'Time range', value: 'time_range' },
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
    const nextNodes = insertAfter(nodes, nodeIndex, {
      kind: 'markdown',
      nodeId: `markdown-${Date.now()}`,
      layoutId: uniqueLayoutNodeId(value.layout ?? [], 'markdown'),
      content: '## New section',
    });
    commitNodes(nextNodes);
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
      node?.kind === 'block'
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
            <Select value={selectedSchema} onValueChange={onSelectedSchemaChange}>
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
  const schema = schemas.find(
    (candidate) => candidate.name === schemaName
  );
  const fields = getSchemaFields(schema);
  const isDatasetBlock = Boolean(block.dataset);

  const update = (patch: Partial<ReportBlockDefinition>) => {
    onChange({ ...block, ...patch });
  };

  const updateType = (type: Exclude<ReportBlockType, 'markdown'>) => {
    onChange(convertBlockType(block, type, fields));
  };

  const updateDataset = (datasetId: string) => {
    const nextDataset = datasets.find((candidate) => candidate.id === datasetId);
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
                  ? dataset?.label ?? block.dataset?.id ?? 'Dataset'
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
                {Object.entries(BLOCK_TYPE_META).map(([type, meta]) => (
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

  const updateQuery = (nextQuery: ReportBlockDatasetQuery) => {
    onChange(reconcileDatasetBlock(block, dataset, nextQuery));
  };

  const updateBlock = (patch: Partial<ReportBlockDefinition>) => {
    onChange({ ...block, ...patch });
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
        <div className="grid gap-2 md:grid-cols-2">
          {columns.map((column, index) => (
            <Field key={column.field} label={`${column.field} label`}>
              <Input
                value={column.label ?? ''}
                placeholder={humanizeFieldName(column.field)}
                onChange={(event) => {
                  const nextColumns = columns.map((current, currentIndex) =>
                    currentIndex === index
                      ? { ...current, label: event.target.value }
                      : current
                  );
                  updateColumns(nextColumns);
                }}
              />
            </Field>
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

function definitionToNodes(definition: ReportDefinition): EditorNode[] {
  if ((definition.layout?.length ?? 0) > 0) {
    const referencedBlockIds = new Set(
      extractLayoutBlockReferences(definition.layout)
    );
    const nodes = (definition.layout ?? []).map(layoutNodeToEditorNode);
    for (const block of definition.blocks) {
      if (!referencedBlockIds.has(block.id)) {
        nodes.push({
          kind: 'block',
          nodeId: `block-${block.id}-appended`,
          layoutId: uniqueLayoutNodeId(
            definition.layout ?? [],
            `${block.id}_node`
          ),
          blockId: block.id,
        });
      }
    }
    return nodes;
  }

  const nodes: EditorNode[] = [];
  const referencedBlockIds = new Set<string>();
  let lastIndex = 0;
  let index = 0;
  BLOCK_PLACEHOLDER_RE.lastIndex = 0;
  let match = BLOCK_PLACEHOLDER_RE.exec(definition.markdown);

  while (match) {
    if (match.index > lastIndex) {
      const content = definition.markdown.slice(lastIndex, match.index);
      if (content.trim().length > 0) {
        nodes.push({
          kind: 'markdown',
          nodeId: `markdown-${index}`,
          layoutId: `markdown_${index + 1}`,
          content: content.trim(),
        });
        index += 1;
      }
    }
    nodes.push({
      kind: 'block',
      nodeId: `block-${match[1]}-${index}`,
      layoutId: `${match[1]}_node`,
      blockId: match[1],
    });
    referencedBlockIds.add(match[1]);
    index += 1;
    lastIndex = match.index + match[0].length;
    match = BLOCK_PLACEHOLDER_RE.exec(definition.markdown);
  }

  if (lastIndex < definition.markdown.length) {
    const content = definition.markdown.slice(lastIndex);
    if (content.trim().length > 0) {
      nodes.push({
        kind: 'markdown',
        nodeId: `markdown-${index}`,
        layoutId: `markdown_${index + 1}`,
        content: content.trim(),
      });
    }
  }

  for (const block of definition.blocks) {
    if (!referencedBlockIds.has(block.id)) {
      nodes.push({
        kind: 'block',
        nodeId: `block-${block.id}-appended`,
        layoutId: `${block.id}_node`,
        blockId: block.id,
      });
    }
  }

  return nodes;
}

function nodesToDefinition(
  definition: ReportDefinition,
  nodes: EditorNode[],
  blocks: ReportBlockDefinition[]
): ReportDefinition {
  const blockById = new Map(blocks.map((block) => [block.id, block]));
  const orderedBlocks: ReportBlockDefinition[] = [];
  const seenBlockIds = new Set<string>();

  for (const blockId of collectEditorNodeBlockIds(nodes)) {
    if (seenBlockIds.has(blockId)) continue;
    const block = blockById.get(blockId);
    if (!block) continue;
    orderedBlocks.push(block);
    seenBlockIds.add(blockId);
  }

  for (const block of blocks) {
    if (!seenBlockIds.has(block.id)) {
      orderedBlocks.push(block);
    }
  }

  return {
    ...definition,
    layout: nodes.map(editorNodeToLayoutNode),
    markdown: nodes
      .flatMap(editorNodeToMarkdownParts)
      .filter((content) => content.length > 0)
      .join('\n\n'),
    blocks: orderedBlocks,
  };
}

function layoutNodeToEditorNode(node: ReportLayoutNode): EditorNode {
  if (node.type === 'markdown') {
    return {
      kind: 'markdown',
      nodeId: `layout-${node.id}`,
      layoutId: node.id,
      content: node.content,
    };
  }
  if (node.type === 'block') {
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
      type: 'markdown',
      content: node.content.trim(),
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

function editorNodeToMarkdownParts(node: EditorNode): string[] {
  if (node.kind === 'markdown') return [node.content.trim()];
  if (node.kind === 'block') return [`{{ block.${node.blockId} }}`];
  if (node.kind === 'metric_row') {
    return [
      node.title ? `## ${node.title}` : '',
      ...node.blocks.map((blockId) => `{{ block.${blockId} }}`),
    ];
  }
  return layoutNodeToMarkdownParts(node.layout);
}

function layoutNodeToMarkdownParts(node: ReportLayoutNode): string[] {
  if (node.type === 'markdown') return [node.content.trim()];
  if (node.type === 'block') return [`{{ block.${node.blockId} }}`];
  if (node.type === 'metric_row') {
    return [
      node.title ? `## ${node.title}` : '',
      ...node.blocks.map((blockId) => `{{ block.${blockId} }}`),
    ];
  }
  if (node.type === 'section') {
    return [
      node.title ? `## ${node.title}` : '',
      node.description ?? '',
      ...(node.children ?? []).flatMap(layoutNodeToMarkdownParts),
    ];
  }
  if (node.type === 'columns') {
    return node.columns.flatMap((column) =>
      (column.children ?? []).flatMap(layoutNodeToMarkdownParts)
    );
  }
  return node.items.map((item) => `{{ block.${item.blockId} }}`);
}

function collectEditorNodeBlockIds(nodes: EditorNode[]): string[] {
  return nodes.flatMap((node) => {
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
    if (node.kind === 'block' && node.blockId === previousBlockId) {
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

function emptyReportSource(): ReportBlockDefinition['source'] {
  return {
    schema: '',
    mode: 'filter',
    groupBy: [],
    aggregates: [],
    orderBy: [],
    filterMappings: [],
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
