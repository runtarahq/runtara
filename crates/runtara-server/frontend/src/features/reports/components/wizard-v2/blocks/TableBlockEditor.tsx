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
  ReportTableColumn,
} from '../../../types';
import { humanizeFieldName } from '../../../utils';

const PLAIN = '__plain__';
const FORMATS: Array<{ value: string; label: string }> = [
  { value: PLAIN, label: 'Plain' },
  { value: 'number', label: 'Number' },
  { value: 'decimal', label: 'Decimal' },
  { value: 'currency', label: 'Currency' },
  { value: 'percent', label: 'Percent' },
  { value: 'date', label: 'Date' },
  { value: 'datetime', label: 'Date + time' },
  { value: 'pill', label: 'Pill' },
];

interface TableBlockEditorProps {
  block: ReportBlockDefinition;
  schemas: Schema[];
  onChange: (block: ReportBlockDefinition) => void;
}

export function TableBlockEditor({
  block,
  schemas,
  onChange,
}: TableBlockEditorProps) {
  const table = block.table ?? { columns: [] };
  const columns = table.columns ?? [];
  const schemaName = block.source?.schema;
  const schema = schemas.find((s) => s.name === schemaName);
  const availableFields =
    schema?.columns.map((column) => column.name) ?? [];

  const updateColumns = (next: ReportTableColumn[]) =>
    onChange({
      ...block,
      table: { ...table, columns: next },
    });

  const addColumn = () => {
    const field = availableFields.find(
      (f) => !columns.some((c) => c.field === f)
    );
    if (!field) return;
    updateColumns([
      ...columns,
      { field, label: humanizeFieldName(field) },
    ]);
  };

  return (
    <div className="grid gap-3">
      <div className="grid gap-1.5">
        <div className="flex items-center justify-between">
          <Label className="text-xs">Columns</Label>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="h-7"
            onClick={addColumn}
            disabled={availableFields.length === 0}
          >
            <Plus className="mr-1 h-3 w-3" /> Add column
          </Button>
        </div>
        {columns.length === 0 ? (
          <p className="text-xs text-muted-foreground">
            No columns yet. Pick a schema, then add columns.
          </p>
        ) : (
          <div className="grid gap-2">
            {columns.map((column, index) => (
              <div
                key={`${column.field}_${index}`}
                className="grid grid-cols-[1fr_1fr_minmax(0,auto)_minmax(0,auto)] items-center gap-2 rounded border p-2"
              >
                <Select
                  value={column.field}
                  onValueChange={(value) =>
                    updateColumns(
                      columns.map((c, i) =>
                        i === index ? { ...c, field: value } : c
                      )
                    )
                  }
                >
                  <SelectTrigger className="h-8 text-xs">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {availableFields.length === 0 ? (
                      <SelectItem disabled value={column.field}>
                        {column.field}
                      </SelectItem>
                    ) : null}
                    {availableFields.map((field) => (
                      <SelectItem key={field} value={field}>
                        {field}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <Input
                  value={column.label ?? ''}
                  placeholder="Label"
                  className="h-8 text-xs"
                  onChange={(event) =>
                    updateColumns(
                      columns.map((c, i) =>
                        i === index
                          ? { ...c, label: event.target.value || null }
                          : c
                      )
                    )
                  }
                />
                <Select
                  value={column.format ?? PLAIN}
                  onValueChange={(value) =>
                    updateColumns(
                      columns.map((c, i) =>
                        i === index
                          ? { ...c, format: value === PLAIN ? null : value }
                          : c
                      )
                    )
                  }
                >
                  <SelectTrigger className="h-8 w-[120px] text-xs">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {FORMATS.map((option) => (
                      <SelectItem key={option.value} value={option.value}>
                        {option.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="h-8 w-8"
                  onClick={() =>
                    updateColumns(columns.filter((_, i) => i !== index))
                  }
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </Button>
              </div>
            ))}
          </div>
        )}
      </div>

      <div className="grid grid-cols-2 gap-3">
        <div className="grid gap-1.5">
          <Label className="text-xs">Default page size</Label>
          <Input
            type="number"
            min={1}
            value={table.pagination?.defaultPageSize ?? ''}
            onChange={(event) => {
              const next = event.target.value
                ? Math.max(1, parseInt(event.target.value, 10))
                : null;
              onChange({
                ...block,
                table: {
                  ...table,
                  pagination: next
                    ? { ...(table.pagination ?? {}), defaultPageSize: next }
                    : undefined,
                },
              });
            }}
          />
        </div>
      </div>
    </div>
  );
}
