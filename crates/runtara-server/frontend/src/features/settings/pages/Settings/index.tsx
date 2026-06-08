import { useState } from 'react';
import { PlusIcon, Key, Ban } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/shared/components/ui/table';
import {
  Breadcrumb,
  ConsoleTableShell,
  ConsoleToolbar,
  StatusPill,
  TableStatusFooter,
  type BreadcrumbItem,
} from '@/shared/components/console';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
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
  const totalKeys = activeKeys.length + revokedKeys.length;
  const hasContent = totalKeys > 0;

  const breadcrumbItems: BreadcrumbItem[] = [
    { label: 'Settings' },
    { label: 'API keys' },
  ];

  const toolbar = (
    <ConsoleToolbar
      left={<Breadcrumb items={breadcrumbItems} />}
      actions={
        // API keys are personal: any user may create their own, so this is not role-gated.
        <Button onClick={() => setCreateOpen(true)} disabled={isError}>
          <PlusIcon className="mr-2 h-4 w-4" />
          New API Key
        </Button>
      }
    />
  );

  const renderBody = () => {
    if (isFetching) {
      return (
        <div className="divide-y divide-border/50">
          {[...Array(4)].map((_, i) => (
            <div key={i} className="flex items-center gap-4 px-5 py-3.5">
              <div className="h-4 w-40 animate-pulse rounded bg-muted/60" />
              <div className="h-4 w-16 animate-pulse rounded bg-muted/60" />
              <div className="h-4 w-28 animate-pulse rounded bg-muted/60" />
              <div className="ml-auto h-4 w-20 animate-pulse rounded bg-muted/60" />
            </div>
          ))}
        </div>
      );
    }

    if (isError) {
      return (
        <div className="flex h-full flex-col items-center justify-center px-6 py-10 text-center">
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
      );
    }

    if (!hasContent) {
      return (
        <div className="flex h-full flex-col items-center justify-center px-6 py-10 text-center">
          <Key className="mb-4 h-10 w-10 text-muted-foreground" />
          <p className="text-base font-semibold text-foreground">
            No API keys yet
          </p>
          <p className="mt-1 text-sm text-muted-foreground">
            Create an API key to connect MCP clients or external integrations.
          </p>
        </div>
      );
    }

    return (
      <Table variant="console">
        <TableHeader>
          <TableRow>
            <TableHead>Name</TableHead>
            <TableHead>Status</TableHead>
            <TableHead>Key</TableHead>
            <TableHead>Created</TableHead>
            <TableHead>Last used</TableHead>
            <TableHead>Expires</TableHead>
            <TableHead className="w-0" />
          </TableRow>
        </TableHeader>
        <TableBody>
          {activeKeys.map((key) => (
            <TableRow key={key.id}>
              <TableCell className="font-medium text-foreground">
                {key.name}
              </TableCell>
              <TableCell className="text-muted-foreground">
                <StatusPill tone="success" label="Active" />
              </TableCell>
              <TableCell className="font-mono text-xs text-muted-foreground">
                {key.key_prefix}...
              </TableCell>
              <TableCell className="text-muted-foreground">
                {formatDate(key.created_at)}
              </TableCell>
              <TableCell className="text-muted-foreground">
                {key.last_used_at ? formatDate(key.last_used_at) : 'Never'}
              </TableCell>
              <TableCell className="text-muted-foreground">
                {key.expires_at ? formatDate(key.expires_at) : 'No expiration'}
              </TableCell>
              <TableCell className="text-right">
                <div className="flex items-center justify-end gap-1">
                  {/* A caller manages only its own keys (server-enforced), so revoke is
                      always available on the keys shown — not role-gated. */}
                  <Button
                    variant="ghost"
                    size="icon"
                    className="h-7 w-7 text-muted-foreground hover:text-destructive"
                    title="Revoke API key"
                    onClick={() => setRevokeTarget(key)}
                  >
                    <Ban className="h-4 w-4" />
                  </Button>
                </div>
              </TableCell>
            </TableRow>
          ))}
          {revokedKeys.map((key) => (
            <TableRow key={key.id} className="opacity-60">
              <TableCell className="font-medium text-foreground">
                {key.name}
              </TableCell>
              <TableCell className="text-muted-foreground">
                <StatusPill tone="neutral" label="Revoked" />
              </TableCell>
              <TableCell className="font-mono text-xs text-muted-foreground">
                {key.key_prefix}...
              </TableCell>
              <TableCell className="text-muted-foreground">
                {formatDate(key.created_at)}
              </TableCell>
              <TableCell className="text-muted-foreground">
                {key.last_used_at ? formatDate(key.last_used_at) : 'Never'}
              </TableCell>
              <TableCell className="text-muted-foreground">
                {key.expires_at ? formatDate(key.expires_at) : 'No expiration'}
              </TableCell>
              <TableCell className="text-right" />
            </TableRow>
          ))}
        </TableBody>
      </Table>
    );
  };

  return (
    <>
      <ConsoleTableShell
        toolbar={toolbar}
        footer={
          hasContent && !isFetching && !isError ? (
            <TableStatusFooter
              left={`${totalKeys} key${totalKeys === 1 ? '' : 's'} · ${activeKeys.length} active · ${revokedKeys.length} revoked`}
            />
          ) : undefined
        }
      >
        {renderBody()}
      </ConsoleTableShell>

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
    </>
  );
}
