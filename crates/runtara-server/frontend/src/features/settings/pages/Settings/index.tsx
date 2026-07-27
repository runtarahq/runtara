import { useState } from 'react';
import { PlusIcon, Key, Ban } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { WithTooltip } from '@/shared/components/ui/tooltip';
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
  ConsoleEmptyState,
  ConsoleErrorState,
  ConsoleTableShell,
  ConsoleToolbar,
  StatusPill,
  TableSkeletonRows,
  TableStatusFooter,
  type BreadcrumbItem,
} from '@/shared/components/console';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
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
          <PlusIcon className="mr-2 size-4" />
          New API Key
        </Button>
      }
    />
  );

  const renderBody = () => {
    if (isFetching) {
      return (
        <TableSkeletonRows
          rows={4}
          widths={['w-40', 'w-16', 'w-28', 'ml-auto w-20']}
        />
      );
    }

    if (isError) {
      return <ConsoleErrorState error={error} entityLabel="API keys" />;
    }

    if (!hasContent) {
      return (
        <ConsoleEmptyState
          icon={<Key className="mb-4 size-10 text-muted-foreground" />}
          title="No API keys yet"
          description="Create an API key to connect MCP clients or external integrations."
        />
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
                  <WithTooltip label="Revoke API key">
                    <Button
                      variant="secondaryDestructive"
                      size="icon-sm"
                      aria-label="Revoke API key"
                      onClick={() => setRevokeTarget(key)}
                    >
                      <Ban className="size-4" />
                    </Button>
                  </WithTooltip>
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
