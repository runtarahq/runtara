import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { Schema } from '@/generated/RuntaraRuntimeApi';
import {
  ReportBlockDefinition,
  ReportBlockType,
  ReportDatasetDefinition,
  ReportSource,
} from '../../../types';
import {
  BLOCK_TYPES,
  blockTypeLabel,
  changeBlockType,
  hasMeaningfulTypeConfig,
} from './changeBlockType';
import { MarkdownBlockEditor } from './MarkdownBlockEditor';
import { TableBlockEditor } from './TableBlockEditor';
import { MetricBlockEditor } from './MetricBlockEditor';
import { ChartBlockEditor } from './ChartBlockEditor';
import { CardBlockEditor } from './CardBlockEditor';
import { ActionsBlockEditor } from './ActionsBlockEditor';
import { FileUploadBlockEditor } from './FileUploadBlockEditor';
import { SourceEditor } from './SourceEditor';
import { VisibilityEditor } from './VisibilityEditor';
import { DatasetReconcileButton } from './DatasetReconcileButton';
import {
  canonicalConditionToReportVisibility,
  reportVisibilityToCanonicalCondition,
} from '../../../utils';

interface BlockEditorProps {
  block: ReportBlockDefinition;
  schemas: Schema[];
  datasets: ReportDatasetDefinition[];
  onChange: (block: ReportBlockDefinition) => void;
}

/** Type-aware block editor. Renders the shared fields (title + source +
 *  visibility) then dispatches to the type-specific section. */
export function BlockEditor({
  block,
  schemas,
  datasets,
  onChange,
}: BlockEditorProps) {
  const hasDataset = Boolean(block.dataset?.id);
  const dataset = hasDataset
    ? datasets.find((d) => d.id === block.dataset?.id)
    : undefined;

  const showSourceEditor =
    block.type !== 'markdown' && block.type !== 'file_upload' && !hasDataset;

  const handleTypeChange = (nextType: ReportBlockType) => {
    if (nextType === block.type) return;
    if (hasMeaningfulTypeConfig(block)) {
      const ok = window.confirm(
        `Switching from "${blockTypeLabel(block.type as ReportBlockType)}" to "${blockTypeLabel(nextType)}" will discard the current ${block.type} configuration. Continue?`
      );
      if (!ok) return;
    }
    onChange(changeBlockType(block, nextType));
  };

  return (
    <div className="grid gap-3">
      <div className="grid gap-3 sm:grid-cols-[1fr_auto]">
        <div className="grid gap-1.5">
          <Label className="text-xs" htmlFor={`title_${block.id}`}>
            Title
          </Label>
          <Input
            id={`title_${block.id}`}
            value={block.title ?? ''}
            placeholder="Optional title shown above the block"
            onChange={(event) =>
              onChange({ ...block, title: event.target.value || null })
            }
          />
        </div>
        <div className="grid gap-1.5">
          <Label className="text-xs" htmlFor={`block_type_${block.id}`}>
            Block type
          </Label>
          <Select
            value={block.type}
            onValueChange={(value) =>
              handleTypeChange(value as ReportBlockType)
            }
          >
            <SelectTrigger
              id={`block_type_${block.id}`}
              className="w-[160px]"
              aria-label="Block type"
              data-testid={`block-type-picker-${block.id}`}
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {BLOCK_TYPES.map((type) => (
                <SelectItem key={type} value={type}>
                  {blockTypeLabel(type)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      </div>

      {showSourceEditor ? (
        <SourceEditor
          source={block.source}
          schemas={schemas}
          onChange={(source: ReportSource) => onChange({ ...block, source })}
        />
      ) : null}

      {hasDataset && dataset ? (
        <div className="flex items-center justify-between rounded border border-dashed p-2 text-xs">
          <span>
            Bound to dataset <code>{dataset.label}</code>.
          </span>
          <DatasetReconcileButton
            block={block}
            dataset={dataset}
            onChange={onChange}
          />
        </div>
      ) : null}

      {block.type === 'markdown' ? (
        <MarkdownBlockEditor block={block} onChange={onChange} />
      ) : null}
      {block.type === 'table' ? (
        <TableBlockEditor block={block} schemas={schemas} onChange={onChange} />
      ) : null}
      {block.type === 'metric' ? (
        <MetricBlockEditor
          block={block}
          schemas={schemas}
          onChange={onChange}
        />
      ) : null}
      {block.type === 'chart' ? (
        <ChartBlockEditor block={block} schemas={schemas} onChange={onChange} />
      ) : null}
      {block.type === 'card' ? (
        <CardBlockEditor block={block} schemas={schemas} onChange={onChange} />
      ) : null}
      {block.type === 'actions' ? (
        <ActionsBlockEditor block={block} onChange={onChange} />
      ) : null}
      {block.type === 'file_upload' ? (
        <FileUploadBlockEditor block={block} onChange={onChange} />
      ) : null}

      <VisibilityEditor
        label="Show when"
        description="Optional canonical condition controlling whether this block renders."
        condition={reportVisibilityToCanonicalCondition(block.showWhen)}
        onChange={(condition) => {
          if (condition === undefined) {
            const { showWhen: _drop, ...rest } = block;
            void _drop;
            onChange(rest as ReportBlockDefinition);
            return;
          }
          const showWhen = canonicalConditionToReportVisibility(condition);
          if (!showWhen) return;
          onChange({ ...block, showWhen });
        }}
      />

      <div className="flex items-center gap-3 text-xs text-muted-foreground">
        <label className="inline-flex items-center gap-1.5">
          <input
            type="checkbox"
            checked={Boolean(block.hideWhenEmpty)}
            onChange={(event) =>
              onChange({ ...block, hideWhenEmpty: event.target.checked })
            }
          />
          Hide when empty
        </label>
        <label className="inline-flex items-center gap-1.5">
          <input
            type="checkbox"
            checked={Boolean(block.lazy)}
            onChange={(event) =>
              onChange({ ...block, lazy: event.target.checked })
            }
          />
          Lazy load
        </label>
      </div>
    </div>
  );
}
