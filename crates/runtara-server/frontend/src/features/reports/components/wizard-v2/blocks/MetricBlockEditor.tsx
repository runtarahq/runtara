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
import { ReportBlockDefinition } from '../../../types';

const PLAIN = '__plain__';
const FORMATS = [
  { value: PLAIN, label: 'Plain' },
  { value: 'number', label: 'Number' },
  { value: 'decimal', label: 'Decimal' },
  { value: 'currency', label: 'Currency' },
  { value: 'percent', label: 'Percent' },
];

interface MetricBlockEditorProps {
  block: ReportBlockDefinition;
  schemas: Schema[];
  onChange: (block: ReportBlockDefinition) => void;
}

export function MetricBlockEditor({
  block,
  schemas,
  onChange,
}: MetricBlockEditorProps) {
  const metric = block.metric ?? { valueField: '' };
  const schemaName = block.source?.schema;
  const schema = schemas.find((s) => s.name === schemaName);
  const fields = schema?.columns.map((c) => c.name) ?? [];

  return (
    <div className="grid gap-3">
      <div className="grid gap-1.5">
        <Label className="text-xs">Value field</Label>
        <Select
          value={metric.valueField || ''}
          onValueChange={(value) =>
            onChange({ ...block, metric: { ...metric, valueField: value } })
          }
        >
          <SelectTrigger className="h-9">
            <SelectValue placeholder="Pick a field" />
          </SelectTrigger>
          <SelectContent>
            {metric.valueField && !fields.includes(metric.valueField) ? (
              <SelectItem disabled value={metric.valueField}>
                {metric.valueField}
              </SelectItem>
            ) : null}
            {fields.map((field) => (
              <SelectItem key={field} value={field}>
                {field}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      <div className="grid grid-cols-2 gap-3">
        <div className="grid gap-1.5">
          <Label className="text-xs">Label</Label>
          <Input
            value={metric.label ?? ''}
            onChange={(event) =>
              onChange({
                ...block,
                metric: {
                  ...metric,
                  label: event.target.value || null,
                },
              })
            }
          />
        </div>

        <div className="grid gap-1.5">
          <Label className="text-xs">Format</Label>
          <Select
            value={metric.format ?? PLAIN}
            onValueChange={(value) =>
              onChange({
                ...block,
                metric: {
                  ...metric,
                  format: value === PLAIN ? null : value,
                },
              })
            }
          >
            <SelectTrigger className="h-9">
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
        </div>
      </div>
    </div>
  );
}
