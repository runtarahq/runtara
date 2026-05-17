import { Schema } from '@/generated/RuntaraRuntimeApi';
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
  ReportSource,
  ReportSourceKind,
  ReportWorkflowRuntimeEntity,
} from '../../../types';

interface SourceEditorProps {
  source: ReportSource | undefined;
  schemas: Schema[];
  onChange: (source: ReportSource) => void;
}

const KIND_OPTIONS: Array<{ value: ReportSourceKind; label: string }> = [
  { value: 'object_model', label: 'Object Model' },
  { value: 'workflow_runtime', label: 'Workflow runtime' },
  { value: 'system', label: 'System' },
];

/** Shared source editor — picks kind, schema/entity, mode. Preserves any
 *  unrecognized source fields (joins, condition, limit, orderBy, etc.) so
 *  the wizard never accidentally drops advanced configuration. */
export function SourceEditor({ source, schemas, onChange }: SourceEditorProps) {
  const kind: ReportSourceKind = source?.kind ?? 'object_model';
  const base = source ?? ({ schema: '' } as ReportSource);

  return (
    <div className="grid gap-3">
      <div className="grid gap-1.5">
        <Label className="text-xs">Source kind</Label>
        <Select
          value={kind}
          onValueChange={(value) =>
            onChange({ ...base, kind: value as ReportSourceKind })
          }
        >
          <SelectTrigger className="h-9">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {KIND_OPTIONS.map((option) => (
              <SelectItem key={option.value} value={option.value}>
                {option.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      {kind === 'object_model' ? (
        <div className="grid gap-1.5">
          <Label className="text-xs">Schema</Label>
          <Select
            value={source?.schema || ''}
            onValueChange={(value) => onChange({ ...base, schema: value })}
          >
            <SelectTrigger className="h-9">
              <SelectValue placeholder="Pick a schema" />
            </SelectTrigger>
            <SelectContent>
              {schemas.map((schema) => (
                <SelectItem key={schema.name} value={schema.name}>
                  {schema.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      ) : (
        <div className="grid gap-1.5">
          <Label className="text-xs">Entity</Label>
          <Input
            value={source?.entity ?? ''}
            placeholder={
              kind === 'workflow_runtime' ? 'instances | actions' : 'metrics'
            }
            onChange={(event) => {
              const raw = event.target.value as ReportWorkflowRuntimeEntity | '';
              onChange({
                ...base,
                entity: raw === '' ? null : raw,
              });
            }}
          />
        </div>
      )}
    </div>
  );
}
