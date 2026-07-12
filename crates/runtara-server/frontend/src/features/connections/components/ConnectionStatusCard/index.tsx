import { useState } from 'react';
import { Loader2, RefreshCw } from 'lucide-react';

import type {
  ConnectionStatus,
  ConnectionTypeDto,
} from '@/generated/RuntaraRuntimeApi';
import { StatusPill } from '@/shared/components/console';
import { Button } from '@/shared/components/ui/button';
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/shared/components/ui/alert-dialog';
import type { FormDefinition } from '@/shared/forms';

import {
  connectionIdentity,
  connectionStatusPill,
} from '@/features/connections/utils/status';

type ConnectionTypeWithForm = ConnectionTypeDto & {
  formDefinition?: FormDefinition;
};

type ConnectionStatusCardProps = {
  status: ConnectionStatus;
  connectionType: ConnectionTypeWithForm;
  /** Readable projection values, source of the identity line. */
  values?: Record<string, unknown>;
  /** Count of stored secrets, for the non-OAuth compact strip. */
  configuredSecretCount: number;
  updatedAt?: string;
  /** Interactive-OAuth type: shows the Connect/Reconnect action. */
  isOAuth: boolean;
  onReconnect?: () => void;
  onSaveAndReconnect?: () => void;
  isReconnecting?: boolean;
  /** Descriptor parameter fields (or cleared secrets) have unsaved edits. */
  hasParamChanges: boolean;
  /** Any unsaved change would reset the provider authorization. */
  hasReauthChanges: boolean;
};

function relativeTime(iso?: string): string | null {
  if (!iso) return null;
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return null;
  const seconds = Math.round((Date.now() - then) / 1000);
  if (seconds < 60) return 'just now';
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  const days = Math.round(hours / 24);
  if (days < 30) return `${days} d ago`;
  return new Date(iso).toLocaleDateString();
}

/** Message shown under the pill for the current status (OAuth types). */
function statusDetail(status: ConnectionStatus, provider: string): string {
  switch (status) {
    case 'REQUIRES_RECONNECTION':
      return "This connection isn't authorized. Your saved credentials are kept — authorize without re-entering them.";
    case 'INVALID_CREDENTIALS':
      return `${provider} rejected the stored credentials. Update them below and save.`;
    case 'UNKNOWN':
      return "Status hasn't been determined yet.";
    default:
      return '';
  }
}

/**
 * Status-first card for the connection editor: pill + identity, and the
 * Connect/Reconnect action for OAuth types. Replaces the amber needs-reconnect
 * and "stored secrets" banners. Reconnect authorizes with the credentials
 * saved on the server, so unsaved parameter edits are guarded first.
 */
export function ConnectionStatusCard({
  status,
  connectionType,
  values,
  configuredSecretCount,
  updatedAt,
  isOAuth,
  onReconnect,
  onSaveAndReconnect,
  isReconnecting,
  hasParamChanges,
  hasReauthChanges,
}: ConnectionStatusCardProps) {
  const [guardOpen, setGuardOpen] = useState(false);
  const pill = connectionStatusPill(status);
  const provider = connectionType.displayName || 'the provider';
  const identity = connectionIdentity(connectionType, values);
  const isConnected = status === 'ACTIVE';
  const reconnectLabel = status === 'REQUIRES_RECONNECTION' ? 'Connect' : 'Reconnect';

  const handleReconnectClick = () => {
    if (hasParamChanges) {
      setGuardOpen(true);
      return;
    }
    onReconnect?.();
  };

  const reconnectButton = isOAuth && onReconnect && (
    <Button
      type="button"
      size="sm"
      variant={isConnected ? 'outline' : 'default'}
      onClick={handleReconnectClick}
      disabled={isReconnecting}
      className={!isConnected ? 'shadow-sm shadow-blue-600/20' : undefined}
    >
      {isReconnecting ? (
        <Loader2 className="w-4 h-4 mr-1.5 animate-spin" />
      ) : (
        <RefreshCw className="w-4 h-4 mr-1.5" />
      )}
      {reconnectLabel}
    </Button>
  );

  const detail = isOAuth ? statusDetail(status, provider) : '';
  const updated = relativeTime(updatedAt);

  return (
    <section className="rounded-lg border border-border/70 bg-card px-4 py-4">
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0 space-y-1.5">
          <div className="flex items-center gap-2 flex-wrap">
            <StatusPill tone={pill.tone} label={pill.label} />
            {identity.map((entry) => (
              <span
                key={entry.label}
                className="text-xs text-muted-foreground"
                title={entry.label}
              >
                {entry.value}
              </span>
            ))}
          </div>
          {detail && (
            <p className="text-xs text-muted-foreground">{detail}</p>
          )}
          {!isOAuth && (
            <p className="text-xs text-muted-foreground">
              {configuredSecretCount > 0
                ? `${configuredSecretCount} secret${configuredSecretCount === 1 ? '' : 's'} configured`
                : 'No secrets configured'}
              {updated ? ` · Updated ${updated}` : ''}
            </p>
          )}
        </div>
        {reconnectButton}
      </div>

      <AlertDialog open={guardOpen} onOpenChange={setGuardOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Save before reconnecting?</AlertDialogTitle>
            <AlertDialogDescription>
              Reconnect authorizes with the credentials that are saved on the
              server. You have unsaved credential changes — reconnecting now
              would use the old values.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter className="gap-2 sm:gap-2">
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            {!hasReauthChanges && onReconnect && (
              <Button
                type="button"
                variant="outline"
                onClick={() => {
                  setGuardOpen(false);
                  onReconnect();
                }}
              >
                Reconnect without saving
              </Button>
            )}
            <AlertDialogAction
              onClick={() => {
                setGuardOpen(false);
                onSaveAndReconnect?.();
              }}
            >
              Save &amp; Reconnect
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </section>
  );
}
