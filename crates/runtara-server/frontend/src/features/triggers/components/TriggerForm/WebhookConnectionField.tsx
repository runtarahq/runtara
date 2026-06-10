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

// The server-side verifier (api/services/webhook_verification.rs) dispatches
// on the connection's integration id; only Mailgun connections currently
// implement signature verification (HMAC-SHA256 over timestamp + token with
// the connection's webhook_signing_key). Other integrations are no-ops, so
// offering them here would be misleading.
const VERIFIABLE_INTEGRATION_IDS = ['mailgun'];

// Radix Select items cannot use an empty-string value, so map "no
// verification" through a sentinel.
const NONE_VALUE = '__none__';

interface WebhookConnectionFieldProps {
  label: string;
  disabled?: boolean;
}

/**
 * Selector for the webhook signature verification connection on HTTP/EMAIL
 * triggers (stored as `configuration.connection_id`).
 */
export function WebhookConnectionField({
  label,
  disabled,
}: WebhookConnectionFieldProps) {
  const { field } = useController({ name: 'webhookConnectionId' });
  const triggerTypeWatch = useWatch({ name: 'triggerType' });
  const { data: connections, isLoading } = useConnections();

  if (triggerTypeWatch !== 'HTTP' && triggerTypeWatch !== 'EMAIL') {
    return null;
  }

  const verifiableConnections = (connections ?? []).filter((c) =>
    VERIFIABLE_INTEGRATION_IDS.includes(c.integrationId ?? '')
  );

  return (
    <div className="space-y-2">
      <FormLabel>{label}</FormLabel>
      {isLoading ? (
        <div className="flex items-center gap-2 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" />
          Loading connections...
        </div>
      ) : verifiableConnections.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          No verifiable connections found. Create a Mailgun connection with a
          webhook signing key to verify incoming webhook signatures.
        </p>
      ) : (
        <>
          <Select
            value={field.value || NONE_VALUE}
            onValueChange={(value) =>
              field.onChange(value === NONE_VALUE ? '' : value)
            }
            disabled={disabled}
          >
            <SelectTrigger>
              <SelectValue placeholder="No verification" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={NONE_VALUE}>No verification</SelectItem>
              {verifiableConnections.map((connection) => (
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
          <p className="text-xs text-muted-foreground">
            Incoming requests are verified against this connection's webhook
            signing key. Only Mailgun connections are currently supported.
          </p>
        </>
      )}
    </div>
  );
}
