import { useState } from 'react';
import { PlusIcon, Key, Clock, Ban } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { Badge } from '@/shared/components/ui/badge';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { TilesPage, TileList } from '@/shared/components/tiles-page';
import { EntityTile } from '@/shared/components/entity-tile';
import { Icons } from '@/shared/components/icons';
import { useApiKeys } from '../../hooks/useApiKeys';
import { CreateApiKeyDialog } from '../../components/CreateApiKeyDialog';
import { RevokeApiKeyDialog } from '../../components/RevokeApiKeyDialog';
import type { ApiKey } from '@/generated/RuntaraRuntimeApi';

function formatDate(dateStr: string | null | undefined) {
  if (!dateStr) return 'Never';
  return new Date(dateStr).toLocaleDateString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

export function Settings() {
  const { data: apiKeys, isFetching, isError, error } = useApiKeys();
  const [createOpen, setCreateOpen] = useState(false);
  const [revokeTarget, setRevokeTarget] = useState<ApiKey | null>(null);

  usePageTitle('Settings');

  const err = error as any;
  const isNetworkError =
    err?.message?.includes('fetch') ||
    err?.code === 'ERR_NETWORK' ||
    !err?.response;

  const activeKeys = apiKeys?.filter((k) => !k.is_revoked) ?? [];
  const revokedKeys = apiKeys?.filter((k) => k.is_revoked) ?? [];

  return (
    <TilesPage
      kicker="Settings"
      title="API Keys"
      action={
        <Button
          className="h-11 w-full rounded-full sm:w-auto sm:px-6"
          onClick={() => setCreateOpen(true)}
          disabled={isError}
        >
          <PlusIcon className="mr-2 h-4 w-4" />
          New API Key
        </Button>
      }
    >
      {isFetching ? (
        <TileList>
          {[...Array(3)].map((_, i) => (
            <div
              key={i}
              className="flex items-center gap-4 rounded-xl bg-muted/20 p-4 animate-pulse"
            >
              <div className="h-9 w-9 rounded-full bg-muted/60" />
              <div className="flex-1 space-y-2">
                <div className="h-4 w-48 rounded bg-muted/60" />
                <div className="h-3 w-72 rounded bg-muted/60" />
              </div>
              <div className="flex gap-2">
                <div className="h-8 w-16 rounded-full bg-muted/60" />
              </div>
            </div>
          ))}
        </TileList>
      ) : isError ? (
        <TileList>
          <div className="flex flex-col items-center justify-center rounded-2xl bg-muted/20 px-6 py-10 text-center">
            <Icons.warning className="mb-4 h-10 w-10 text-destructive" />
            <p className="text-base font-semibold text-foreground">
              {isNetworkError
                ? 'Unable to connect to backend'
                : 'An error occurred'}
            </p>
            <p className="mt-1 text-sm text-muted-foreground">
              {isNetworkError
                ? 'Please check that the backend service is running and try again.'
                : 'There was a problem loading API keys. Please try again.'}
            </p>
          </div>
        </TileList>
      ) : activeKeys.length === 0 && revokedKeys.length === 0 ? (
        <TileList>
          <div className="flex flex-col items-center justify-center rounded-2xl bg-muted/20 px-6 py-10 text-center">
            <Key className="mb-4 h-10 w-10 text-muted-foreground/50" />
            <p className="text-base font-semibold text-foreground">
              No API keys yet
            </p>
            <p className="mt-1 text-sm text-muted-foreground">
              Create an API key to connect MCP clients or external integrations.
            </p>
          </div>
        </TileList>
      ) : (
        <TileList>
          {activeKeys.map((key) => (
            <EntityTile
              key={key.id}
              title={key.name}
              badges={
                <Badge
                  variant="outline"
                  className="text-emerald-600 border-emerald-300 dark:text-emerald-400 dark:border-emerald-700"
                >
                  Active
                </Badge>
              }
              description={
                <span className="font-mono text-xs">{key.key_prefix}...</span>
              }
              metadata={[
                <>
                  <Clock className="h-3 w-3" />
                  Created {formatDate(key.created_at)}
                </>,
                key.last_used_at ? (
                  <>
                    <Clock className="h-3 w-3" />
                    Last used {formatDate(key.last_used_at)}
                  </>
                ) : (
                  'Never used'
                ),
                key.expires_at ? (
                  <>Expires {formatDate(key.expires_at)}</>
                ) : (
                  'No expiration'
                ),
              ]}
              actions={
                <Button
                  variant="ghost"
                  size="sm"
                  className="text-destructive hover:text-destructive hover:bg-destructive/10"
                  onClick={() => setRevokeTarget(key)}
                >
                  <Ban className="mr-1 h-3.5 w-3.5" />
                  Revoke
                </Button>
              }
            />
          ))}

          {revokedKeys.length > 0 && (
            <>
              <div className="pt-4">
                <p className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
                  Revoked
                </p>
              </div>
              {revokedKeys.map((key) => (
                <EntityTile
                  key={key.id}
                  title={key.name}
                  className="opacity-60"
                  badges={<Badge variant="secondary">Revoked</Badge>}
                  description={
                    <span className="font-mono text-xs">
                      {key.key_prefix}...
                    </span>
                  }
                  metadata={[
                    <>
                      <Clock className="h-3 w-3" />
                      Created {formatDate(key.created_at)}
                    </>,
                  ]}
                />
              ))}
            </>
          )}
        </TileList>
      )}

      <CreateApiKeyDialog
        open={createOpen}
        onClose={() => setCreateOpen(false)}
      />

      <RevokeApiKeyDialog
        open={!!revokeTarget}
        keyId={revokeTarget?.id ?? null}
        keyName={revokeTarget?.name ?? ''}
        onClose={() => setRevokeTarget(null)}
      />
    </TilesPage>
  );
}
