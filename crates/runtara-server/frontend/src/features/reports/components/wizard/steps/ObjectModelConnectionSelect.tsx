import { ConnectionDto } from '@/generated/RuntaraRuntimeApi';
import { Label } from '@/shared/components/ui/label';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';

const REPORT_DEFAULT_VALUE = '__report_default__';

export function ObjectModelConnectionSelect({
  label = 'Connection',
  value,
  connections,
  defaultConnectionId,
  onChange,
}: {
  label?: string;
  value?: string | null;
  connections: ConnectionDto[];
  defaultConnectionId?: string | null;
  onChange: (connectionId: string | undefined) => void;
}) {
  const defaultConnection = connections.find(
    (connection) => connection.id === defaultConnectionId
  );

  return (
    <div className="grid gap-1.5">
      <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
        {label}
      </Label>
      <Select
        value={value || REPORT_DEFAULT_VALUE}
        onValueChange={(next) =>
          onChange(next === REPORT_DEFAULT_VALUE ? undefined : next)
        }
      >
        <SelectTrigger>
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={REPORT_DEFAULT_VALUE}>
            Report default
            {defaultConnection ? ` (${defaultConnection.title})` : ''}
          </SelectItem>
          {connections.map((connection) => (
            <SelectItem key={connection.id} value={connection.id}>
              {connection.title}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );
}
