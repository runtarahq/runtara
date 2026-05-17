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
import { Plus, Trash2 } from 'lucide-react';
import { Schema } from '@/generated/RuntaraRuntimeApi';
import {
  ReportBlockDefinition,
  ReportCardField,
  ReportCardGroup,
} from '../../../types';

interface CardBlockEditorProps {
  block: ReportBlockDefinition;
  schemas: Schema[];
  onChange: (block: ReportBlockDefinition) => void;
}

/** Editor for the simple case: one group with a flat list of fields. Cards
 *  with multiple groups, subcards, or subtables preserve their shape but the
 *  editor flags them as "complex card" and surfaces a "switch to legacy
 *  wizard" hint at the parent level. */
export function CardBlockEditor({
  block,
  schemas,
  onChange,
}: CardBlockEditorProps) {
  const card = block.card ?? { groups: [] };
  const groups: ReportCardGroup[] = card.groups ?? [];
  const schema = schemas.find((s) => s.name === block.source?.schema);
  const availableFields = schema?.columns.map((c) => c.name) ?? [];

  const updateGroups = (next: ReportCardGroup[]) =>
    onChange({ ...block, card: { ...card, groups: next } });

  const ensurePrimaryGroup = (): ReportCardGroup =>
    groups[0] ?? { id: `group_${block.id}`, fields: [] };

  const updatePrimaryFields = (fields: ReportCardField[]) => {
    const primary = { ...ensurePrimaryGroup(), fields };
    const next = groups.length === 0 ? [primary] : [primary, ...groups.slice(1)];
    updateGroups(next);
  };

  const isComplex =
    groups.length > 1 ||
    groups[0]?.fields?.some((f) => f.kind === 'subcard' || f.kind === 'subtable');

  const fields = groups[0]?.fields ?? [];

  return (
    <div className="grid gap-3">
      {isComplex ? (
        <p className="text-xs text-amber-700 dark:text-amber-300">
          This card uses multiple groups, subcards, or subtables. Edit those in
          the legacy wizard; the simple field list below shows the first
          group's fields.
        </p>
      ) : null}

      <div className="grid gap-1.5">
        <div className="flex items-center justify-between">
          <Label className="text-xs">Fields</Label>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="h-7"
            onClick={() => {
              const field = availableFields.find(
                (f) => !fields.some((existing) => existing.field === f)
              );
              if (!field) return;
              updatePrimaryFields([
                ...fields,
                { field, kind: 'value' as const },
              ]);
            }}
            disabled={availableFields.length === 0}
          >
            <Plus className="mr-1 h-3 w-3" /> Add field
          </Button>
        </div>

        {fields.length === 0 ? (
          <p className="text-xs text-muted-foreground">
            No fields yet. Pick a schema, then add fields.
          </p>
        ) : (
          <div className="grid gap-2">
            {fields.map((field, index) => (
              <div
                key={`${field.field ?? index}_${index}`}
                className="grid grid-cols-[1fr_1fr_minmax(0,auto)] items-center gap-2 rounded border p-2"
              >
                <Select
                  value={field.field || ''}
                  onValueChange={(value) =>
                    updatePrimaryFields(
                      fields.map((f, i) =>
                        i === index ? { ...f, field: value } : f
                      )
                    )
                  }
                >
                  <SelectTrigger className="h-8 text-xs">
                    <SelectValue placeholder="Field" />
                  </SelectTrigger>
                  <SelectContent>
                    {field.field && !availableFields.includes(field.field) ? (
                      <SelectItem disabled value={field.field}>
                        {field.field}
                      </SelectItem>
                    ) : null}
                    {availableFields.map((option) => (
                      <SelectItem key={option} value={option}>
                        {option}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <Input
                  value={field.label ?? ''}
                  placeholder="Label"
                  className="h-8 text-xs"
                  onChange={(event) =>
                    updatePrimaryFields(
                      fields.map((f, i) =>
                        i === index
                          ? { ...f, label: event.target.value || null }
                          : f
                      )
                    )
                  }
                />
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="h-8 w-8"
                  onClick={() =>
                    updatePrimaryFields(fields.filter((_, i) => i !== index))
                  }
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </Button>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
