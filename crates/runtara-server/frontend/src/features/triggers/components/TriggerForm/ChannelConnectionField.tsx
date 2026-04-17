import { useController, useWatch } from 'react-hook-form';
import { Loader2 } from 'lucide-react';
import { FormLabel } from '@/shared/components/ui/form';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { useConnections } from '@/features/connections/hooks/useConnections';

const CHANNEL_INTEGRATION_IDS = [
  'telegram_bot',
  'slack_bot',
  'teams_bot',
  'mailgun',
];

interface ChannelConnectionFieldProps {
  label: string;
  disabled?: boolean;
}

export function ChannelConnectionField({
  label,
  disabled,
}: ChannelConnectionFieldProps) {
  const { field } = useController({ name: 'connectionId' });
  const triggerTypeWatch = useWatch({ name: 'triggerType' });
  const { data: connections, isLoading } = useConnections();

  if (triggerTypeWatch !== 'CHANNEL') {
    return null;
  }

  const channelConnections = (connections ?? []).filter((c) =>
    CHANNEL_INTEGRATION_IDS.includes(c.integrationId ?? '')
  );

  return (
    <div className="space-y-2">
      <FormLabel>{label}</FormLabel>
      {isLoading ? (
        <div className="flex items-center gap-2 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" />
          Loading connections...
        </div>
      ) : channelConnections.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          No channel connections found. Create a Telegram Bot or other messaging
          connection first.
        </p>
      ) : (
        <Select
          value={field.value || ''}
          onValueChange={field.onChange}
          disabled={disabled}
        >
          <SelectTrigger>
            <SelectValue placeholder="Select a channel connection" />
          </SelectTrigger>
          <SelectContent>
            {channelConnections.map((connection) => (
              <SelectItem key={connection.id} value={connection.id}>
                {connection.title}
                {connection.integrationId && (
                  <span className="ml-2 text-xs text-muted-foreground">
                    ({connection.integrationId.replace(/_/g, ' ')})
                  </span>
                )}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )}
    </div>
  );
}
