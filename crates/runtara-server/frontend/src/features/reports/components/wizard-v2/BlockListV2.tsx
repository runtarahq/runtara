import { useState } from 'react';
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
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from '@/shared/components/ui/card';
import {
  ChevronDown,
  ChevronUp,
  Plus,
  Trash2,
} from 'lucide-react';
import { Schema } from '@/generated/RuntaraRuntimeApi';
import {
  ReportBlockDefinition,
  ReportBlockType,
  ReportDatasetDefinition,
  ReportDefinition,
} from '../../types';
import { BlockEditor } from './blocks/BlockEditor';
import {
  addBlock,
  makeBlockId,
  moveBlock,
  orderedBlocksFromDefinition,
  removeBlock,
  updateBlock,
} from './layoutOps';

interface BlockListV2Props {
  definition: ReportDefinition;
  schemas: Schema[];
  onChange: (definition: ReportDefinition) => void;
}

const BLOCK_TYPES: Array<{ value: ReportBlockType; label: string }> = [
  { value: 'markdown', label: 'Text' },
  { value: 'metric', label: 'Metric' },
  { value: 'chart', label: 'Chart' },
  { value: 'table', label: 'Table' },
  { value: 'card', label: 'Card' },
  { value: 'actions', label: 'Actions' },
];

function newBlock(type: ReportBlockType, title: string): ReportBlockDefinition {
  const id = makeBlockId(title);
  const base: ReportBlockDefinition = {
    id,
    type,
    title: title || null,
    source: { schema: '' },
  };
  if (type === 'markdown') return { ...base, markdown: { content: '' } };
  if (type === 'table') return { ...base, table: { columns: [] } };
  if (type === 'metric') return { ...base, metric: { valueField: '' } };
  if (type === 'chart')
    return { ...base, chart: { kind: 'bar', x: '', series: [] } };
  if (type === 'card') return { ...base, card: { groups: [] } };
  if (type === 'actions') return { ...base, actions: {} };
  return base;
}

export function BlockListV2({
  definition,
  schemas,
  onChange,
}: BlockListV2Props) {
  const blocks = orderedBlocksFromDefinition(definition);
  const datasets: ReportDatasetDefinition[] = definition.datasets ?? [];
  const [openId, setOpenId] = useState<string | null>(null);
  const [newType, setNewType] = useState<ReportBlockType>('markdown');
  const [newTitle, setNewTitle] = useState('');

  const handleAdd = () => {
    const block = newBlock(newType, newTitle.trim() || 'New block');
    onChange(addBlock(definition, block));
    setNewTitle('');
    setOpenId(block.id);
  };

  return (
    <div className="grid gap-4">
      {blocks.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          No blocks yet. Add one below.
        </p>
      ) : (
        <div className="grid gap-3">
          {blocks.map((block, index) => {
            const isOpen = openId === block.id;
            return (
              <Card key={block.id}>
                <CardHeader className="flex flex-row items-center justify-between gap-2 space-y-0 py-3">
                  <button
                    type="button"
                    className="flex items-center gap-2 text-left"
                    onClick={() => setOpenId(isOpen ? null : block.id)}
                  >
                    {isOpen ? (
                      <ChevronUp className="h-3.5 w-3.5" />
                    ) : (
                      <ChevronDown className="h-3.5 w-3.5" />
                    )}
                    <CardTitle className="text-sm">
                      {block.title || block.id}
                    </CardTitle>
                    <span className="rounded bg-muted px-1.5 py-0.5 text-xs text-muted-foreground">
                      {block.type}
                    </span>
                  </button>
                  <div className="flex items-center gap-1">
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      className="h-7 w-7"
                      disabled={index === 0}
                      onClick={() =>
                        onChange(moveBlock(definition, block.id, index - 1))
                      }
                    >
                      <ChevronUp className="h-3.5 w-3.5" />
                    </Button>
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      className="h-7 w-7"
                      disabled={index === blocks.length - 1}
                      onClick={() =>
                        onChange(moveBlock(definition, block.id, index + 1))
                      }
                    >
                      <ChevronDown className="h-3.5 w-3.5" />
                    </Button>
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      className="h-7 w-7 text-destructive"
                      onClick={() => {
                        if (openId === block.id) setOpenId(null);
                        onChange(removeBlock(definition, block.id));
                      }}
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </Button>
                  </div>
                </CardHeader>
                {isOpen ? (
                  <CardContent className="pt-0">
                    <BlockEditor
                      block={block}
                      schemas={schemas}
                      datasets={datasets}
                      onChange={(next) =>
                        onChange(
                          updateBlock(definition, block.id, () => next)
                        )
                      }
                    />
                  </CardContent>
                ) : null}
              </Card>
            );
          })}
        </div>
      )}

      <div className="rounded-lg border bg-muted/30 p-3">
        <div className="grid grid-cols-[minmax(0,1fr)_180px_minmax(0,auto)] items-end gap-2">
          <div className="grid gap-1.5">
            <Label className="text-xs">New block title</Label>
            <Input
              value={newTitle}
              placeholder="Untitled block"
              onChange={(event) => setNewTitle(event.target.value)}
            />
          </div>
          <div className="grid gap-1.5">
            <Label className="text-xs">Type</Label>
            <Select
              value={newType}
              onValueChange={(value) =>
                setNewType(value as ReportBlockType)
              }
            >
              <SelectTrigger className="h-9">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {BLOCK_TYPES.map((option) => (
                  <SelectItem key={option.value} value={option.value}>
                    {option.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <Button type="button" onClick={handleAdd} className="h-9">
            <Plus className="mr-1 h-3.5 w-3.5" /> Add block
          </Button>
        </div>
      </div>
    </div>
  );
}
