import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { Switch } from '@/shared/components/ui/switch';
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/shared/components/ui/table';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/shared/components/ui/popover';
import { TagInput } from '@/shared/components/ui/tag-input';
import { Plus, Trash2, Settings2 } from 'lucide-react';

export type FieldDefinition = {
  __id: string;
  name: string;
  dataType:
    | 'string'
    | 'integer'
    | 'boolean'
    | 'decimal'
    | 'timestamp'
    | 'json'
    | 'enum';
  nullable: boolean;
  unique: boolean;
  default?: string;
  precision?: number;
  scale?: number;
  values?: string[];
};

const FIELD_TYPES = [
  { value: 'string', label: 'String', abbr: 'str' },
  { value: 'integer', label: 'Integer', abbr: 'int' },
  { value: 'decimal', label: 'Decimal', abbr: 'dec' },
  { value: 'boolean', label: 'Boolean', abbr: 'bool' },
  { value: 'timestamp', label: 'Timestamp', abbr: 'ts' },
  { value: 'json', label: 'JSON', abbr: 'json' },
  { value: 'enum', label: 'Enum', abbr: 'enum' },
];

const getTypeAbbreviation = (type: string): string => {
  const found = FIELD_TYPES.find((t) => t.value === type);
  return found?.abbr || type;
};

const getTypeLabel = (type: string): string => {
  const found = FIELD_TYPES.find((t) => t.value === type);
  return found?.label || type;
};

interface FieldRowProps {
  field: FieldDefinition;
  onUpdate: <K extends keyof FieldDefinition>(
    key: K,
    value: FieldDefinition[K]
  ) => void;
  onRemove: () => void;
}

function FieldRow({ field, onUpdate, onRemove }: FieldRowProps) {
  const hasConfig = field.dataType === 'decimal' || field.dataType === 'enum';

  return (
    <TableRow className="hover:bg-muted/30 group">
      {/* Type column */}
      <TableCell className="align-middle">
        <span
          className="text-[11px] font-mono px-1.5 py-0.5 rounded text-muted-foreground bg-muted/40 cursor-default"
          title={getTypeLabel(field.dataType)}
        >
          {getTypeAbbreviation(field.dataType)}
        </span>
      </TableCell>

      {/* Name column */}
      <TableCell className="align-middle">
        <Input
          value={field.name}
          onChange={(e) => onUpdate('name', e.target.value)}
          placeholder="column_name"
          className="h-9 rounded-lg font-mono text-sm"
        />
      </TableCell>

      {/* Data Type column */}
      <TableCell className="align-middle">
        <Select
          value={field.dataType}
          onValueChange={(value: FieldDefinition['dataType']) =>
            onUpdate('dataType', value)
          }
        >
          <SelectTrigger className="h-9 rounded-lg w-full">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {FIELD_TYPES.map((type) => (
              <SelectItem key={type.value} value={type.value}>
                {type.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </TableCell>

      {/* Required column */}
      <TableCell className="align-middle text-center">
        <div
          className="flex justify-center"
          title={field.nullable ? 'Optional' : 'Required'}
        >
          <Switch
            checked={!field.nullable}
            onCheckedChange={(checked) => onUpdate('nullable', !checked)}
          />
        </div>
      </TableCell>

      {/* Unique column */}
      <TableCell className="align-middle text-center">
        <div
          className="flex justify-center"
          title={field.unique ? 'Unique constraint' : 'No unique constraint'}
        >
          <Switch
            checked={field.unique}
            onCheckedChange={(checked) => onUpdate('unique', checked)}
          />
        </div>
      </TableCell>

      {/* Default column */}
      <TableCell className="align-middle">
        <Input
          value={field.default || ''}
          onChange={(e) => onUpdate('default', e.target.value || undefined)}
          placeholder="—"
          className="h-9 rounded-lg text-sm"
        />
      </TableCell>

      {/* Actions column */}
      <TableCell className="align-middle">
        <div className="flex items-center justify-end gap-1">
          {hasConfig && (
            <Popover>
              <PopoverTrigger asChild>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="h-8 w-8 text-muted-foreground hover:text-foreground"
                >
                  <Settings2 className="h-4 w-4" />
                </Button>
              </PopoverTrigger>
              <PopoverContent className="w-80" align="end">
                {field.dataType === 'decimal' && (
                  <div className="space-y-4">
                    <div className="flex items-center gap-2">
                      <Settings2 className="h-4 w-4 text-muted-foreground" />
                      <span className="font-medium text-sm">
                        Decimal Configuration
                      </span>
                    </div>
                    <div className="grid grid-cols-2 gap-3">
                      <div className="space-y-1.5">
                        <label className="text-xs font-medium text-muted-foreground">
                          Precision
                        </label>
                        <Input
                          type="number"
                          min="1"
                          max="1000"
                          value={field.precision || 19}
                          onChange={(e) =>
                            onUpdate(
                              'precision',
                              parseInt(e.target.value) || 19
                            )
                          }
                          className="h-9"
                        />
                        <p className="text-[10px] text-muted-foreground">
                          Total digits (1-1000)
                        </p>
                      </div>
                      <div className="space-y-1.5">
                        <label className="text-xs font-medium text-muted-foreground">
                          Scale
                        </label>
                        <Input
                          type="number"
                          min="0"
                          max={field.precision || 19}
                          value={field.scale || 4}
                          onChange={(e) =>
                            onUpdate('scale', parseInt(e.target.value) || 4)
                          }
                          className="h-9"
                        />
                        <p className="text-[10px] text-muted-foreground">
                          Decimal places
                        </p>
                      </div>
                    </div>
                  </div>
                )}
                {field.dataType === 'enum' && (
                  <div className="space-y-4">
                    <div className="flex items-center gap-2">
                      <Settings2 className="h-4 w-4 text-muted-foreground" />
                      <span className="font-medium text-sm">Enum Values</span>
                    </div>
                    <TagInput
                      value={field.values || []}
                      onChange={(values) => onUpdate('values', values)}
                      placeholder="Type value and press Enter"
                    />
                    <p className="text-xs text-muted-foreground">
                      Add allowed values. Press Enter after each.
                    </p>
                  </div>
                )}
              </PopoverContent>
            </Popover>
          )}
          <Button
            type="button"
            variant="ghost"
            size="icon"
            onClick={onRemove}
            className="h-8 w-8 text-muted-foreground hover:text-destructive opacity-0 group-hover:opacity-100 transition-opacity"
            title="Remove column"
          >
            <Trash2 className="h-4 w-4" />
          </Button>
        </div>
      </TableCell>
    </TableRow>
  );
}

interface ObjectSchemaFieldsTableProps {
  fields: FieldDefinition[];
  onFieldsChange: (fields: FieldDefinition[]) => void;
  onAddField: () => void;
}

export function ObjectSchemaFieldsTable({
  fields,
  onFieldsChange,
  onAddField,
}: ObjectSchemaFieldsTableProps) {
  const updateField = <K extends keyof FieldDefinition>(
    index: number,
    key: K,
    value: FieldDefinition[K]
  ) => {
    const newFields = [...fields];
    newFields[index] = {
      ...newFields[index],
      [key]: value,
    };
    onFieldsChange(newFields);
  };

  const removeField = (index: number) => {
    onFieldsChange(fields.filter((_, idx) => idx !== index));
  };

  if (fields.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center rounded-2xl border border-dashed border-border/50 bg-muted/20 px-6 py-10 text-center">
        <p className="text-base font-medium text-foreground">No columns yet</p>
        <p className="mt-1 text-sm text-muted-foreground">
          Add at least one column to describe the shape of this schema
        </p>
        <Button
          type="button"
          variant="outline"
          className="mt-4 h-10 px-4"
          onClick={onAddField}
        >
          <Plus className="mr-2 h-4 w-4" />
          Add your first column
        </Button>
      </div>
    );
  }

  return (
    <div className="space-y-4">
      <div className="rounded-xl bg-card overflow-hidden">
        <Table className="table-fixed w-full">
          <colgroup>
            <col className="w-14" />
            <col className="w-40" />
            <col className="w-32" />
            <col className="w-20" />
            <col className="w-20" />
            <col className="w-32" />
            <col className="w-20" />
          </colgroup>
          <TableHeader>
            <TableRow className="hover:bg-transparent border-b border-border/40">
              <TableHead className="text-xs font-medium text-muted-foreground">
                Type
              </TableHead>
              <TableHead className="text-xs font-medium text-muted-foreground">
                Name
              </TableHead>
              <TableHead className="text-xs font-medium text-muted-foreground">
                Data Type
              </TableHead>
              <TableHead
                className="text-xs font-medium text-muted-foreground text-center"
                title="Required - Every record must have a value"
              >
                Req
              </TableHead>
              <TableHead
                className="text-xs font-medium text-muted-foreground text-center"
                title="Unique - No duplicate values allowed"
              >
                Uniq
              </TableHead>
              <TableHead className="text-xs font-medium text-muted-foreground">
                Default
              </TableHead>
              <TableHead />
            </TableRow>
          </TableHeader>
          <TableBody>
            {fields.map((field, index) => (
              <FieldRow
                key={field.__id}
                field={field}
                onUpdate={(key, value) => updateField(index, key, value)}
                onRemove={() => removeField(index)}
              />
            ))}
          </TableBody>
        </Table>
      </div>

      <div className="flex justify-end">
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="text-muted-foreground hover:text-foreground"
          onClick={onAddField}
        >
          <Plus className="h-4 w-4 mr-2" />
          Add column
        </Button>
      </div>
    </div>
  );
}
