import { useMemo, useState } from 'react';
import { flushSync } from 'react-dom';
import { useNavigate, useParams } from 'react-router';
import { useAuth } from 'react-oidc-context';
import { toast } from 'sonner';
import { useCustomMutation, useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { Loader2 } from '@/shared/components/loader.tsx';
import { ConnectionTypeDto } from '@/generated/RuntaraRuntimeApi';
import {
  DynamicConnectionForm,
  type ConnectionFormOperations,
} from '@/features/connections/components/Forms/DynamicConnectionForm';
import { type EditProjection } from '@/features/connections/components/Forms/DynamicConnectionForm/adapter';
import {
  getConnectionById,
  getConnectionTypes,
  updateConnection,
  type UpdateConnectionInput,
  removeConnection,
} from '@/features/connections/queries';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { useConnectionRateLimitStatus } from '@/features/analytics/hooks/useRateLimits';
import { useConnectionOAuth } from '@/features/connections/hooks/useConnectionOAuth';
import { useNavigationBlockerStore } from '@/shared/stores/navigationBlockerStore';
import { queryClient } from '@/main.tsx';
import type { FormDefinition } from '@/shared/forms';
import { buildConnectionUpdateInput } from './update-payload';
import { ConnectionConflictNotice } from './ConnectionConflictNotice';

type ConnectionRecord = Awaited<ReturnType<typeof getConnectionById>>;

interface ConnectionConflictState {
  message: string;
  pending: UpdateConnectionInput;
  latest?: ConnectionRecord;
  loadingLatest: boolean;
}

function changedReadableFields(
  opened: unknown,
  latest: unknown
): string[] {
  const before = opened as {
    title?: unknown;
    editProjection?: EditProjection;
  };
  const after = latest as {
    title?: unknown;
    editProjection?: EditProjection;
  };
  const changed: string[] = [];
  if (before.title !== after.title) changed.push('Title');
  const beforeValues = before.editProjection?.values ?? {};
  const afterValues = after.editProjection?.values ?? {};
  for (const name of new Set([
    ...Object.keys(beforeValues),
    ...Object.keys(afterValues),
  ])) {
    if (
      JSON.stringify(beforeValues[name]) !== JSON.stringify(afterValues[name])
    ) {
      changed.push(name.replace(/_/g, ' '));
    }
  }
  return changed;
}

export function Connection() {
  const { id } = useParams();

  const navigate = useNavigate();
  const auth = useAuth();
  const [conflict, setConflict] = useState<ConnectionConflictState | null>(
    null
  );

  const connection = useCustomQuery({
    queryKey: queryKeys.connections.byId(id ?? ''),
    queryFn: (token: string) => getConnectionById(token, id!),
    enabled: !!id,
  });

  const connectionTypesQuery = useCustomQuery({
    queryKey: queryKeys.connections.types(),
    queryFn: getConnectionTypes,
  });

  // Live rate-limit status for the honest protection badge in the form.
  const rateLimitStatusQuery = useConnectionRateLimitStatus(id);

  // Interactive-OAuth reconnect: re-runs the authorize popup using the
  // connection's stored credentials (no re-entry).
  const { authorize, isAuthorizing } = useConnectionOAuth();

  const mutation = useCustomMutation({
    mutationFn: updateConnection,
    suppressConflictToasts: true,
    onSuccess: () => {
      setConflict(null);
      queryClient.invalidateQueries({ queryKey: queryKeys.connections.all });
      queryClient.invalidateQueries({
        queryKey: queryKeys.connections.byId(id ?? ''),
      });
      toast.success('Connection saved.');
    },
    onError: (error, variables) => {
      if (error.response?.status !== 409 || !id) return;
      const message =
        error.response.data?.message ??
        error.response.data?.error ??
        'This connection changed after you opened it.';
      setConflict({ message, pending: variables, loadingLatest: true });
      void getConnectionById(auth.user?.access_token ?? '', id)
        .then((latest) => {
          setConflict((current) =>
            current?.pending === variables
              ? { ...current, latest, loadingLatest: false }
              : current
          );
        })
        .catch(() => {
          setConflict((current) =>
            current?.pending === variables
              ? {
                  ...current,
                  message: `${current.message} The latest version could not be loaded; try again.`,
                  loadingLatest: false,
                }
              : current
          );
        });
    },
  });

  const deleteMutation = useCustomMutation({
    mutationFn: removeConnection,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.connections.all });
      // The row is gone: leaving must not trip the unsaved-changes blocker.
      // flushSync forces useBlocker to re-subscribe with shouldBlock=false
      // before the navigate below runs.
      flushSync(() => {
        useNavigationBlockerStore.getState().setBlocker(false);
      });
      navigate('/connections');
      toast.success(
        `Connection "${connection.data?.title ?? 'connection'}" deleted.`
      );
    },
  });

  const currentConnectionType = useMemo(() => {
    const connData = connection.data as any;
    if (!connData?.integrationId || !connectionTypesQuery.data) {
      return null;
    }
    return (
      connectionTypesQuery.data.find(
        (ct: ConnectionTypeDto) => ct.integrationId === connData.integrationId
      ) || null
    );
  }, [connectionTypesQuery.data, connection.data]);

  // Set page title with connection name
  usePageTitle(
    connection.data?.title
      ? `Connections - ${connection.data.title}`
      : 'Edit Connection'
  );

  const handleSubmit = (
    data: Record<string, unknown>,
    operations: ConnectionFormOperations
  ) => {
    const descriptor = (
      currentConnectionType as ConnectionTypeDto & {
        formDefinition?: FormDefinition;
      }
    ).formDefinition;
    const projection = (
      connection.data as unknown as {
        editProjection?: EditProjection;
      }
    ).editProjection;
    if (!descriptor || !projection?.version) {
      toast.error(
        'Connection edit metadata is unavailable. Reload and try again.'
      );
      return;
    }
    const update = buildConnectionUpdateInput({
      id: id as string,
      data,
      dirtyFieldNames: operations.dirtyFields,
      clearSecrets: operations.clearSecrets,
      definition: descriptor,
      projection,
    });
    mutation.mutate(update);
  };

  // Show loading indicator while fetching initial data
  if (connection.isLoading || connectionTypesQuery.isLoading) {
    return (
      <div className="mx-auto max-w-5xl px-4 py-10 sm:px-6 lg:px-10">
        <Loader2 />
      </div>
    );
  }

  if (!currentConnectionType || !connection.data) {
    return (
      <div className="mx-auto max-w-5xl px-4 py-10 sm:px-6 lg:px-10">
        <div className="rounded-lg bg-muted/20 px-6 py-8 text-center text-muted-foreground">
          <p>Connection not found or configuration unavailable.</p>
        </div>
      </div>
    );
  }

  const handleDelete = () => {
    if (id) {
      deleteMutation.mutate(id);
    }
  };

  // Only interactive authorization-code types (those with an OAuthConfig) can be
  // re-authorized via a popup; client-credentials / API-key types mint or carry
  // their own credentials and have nothing to reconnect.
  const isOAuthAuthCode = !!(
    currentConnectionType as unknown as {
      oauthConfig?: unknown;
    }
  )?.oauthConfig;
  const needsReconnect =
    (connection.data as { status?: string })?.status ===
    'REQUIRES_RECONNECTION';
  const conflictChanges = conflict?.latest
    ? changedReadableFields(connection.data, conflict.latest)
    : [];
  const latestVersion = (
    conflict?.latest as unknown as { editProjection?: EditProjection }
  )?.editProjection?.version;
  const conflictNotice = conflict ? (
    <ConnectionConflictNotice
      message={conflict.message}
      loadingLatest={conflict.loadingLatest}
      changedFields={conflictChanges}
      canRecover={Boolean(conflict.latest && latestVersion)}
      applying={mutation.isPending}
      onReload={() => {
        if (!conflict.latest || !id) return;
        queryClient.setQueryData(
          queryKeys.connections.byId(id),
          conflict.latest
        );
        setConflict(null);
      }}
      onReapply={() => {
        if (!latestVersion) return;
        mutation.mutate({
          ...conflict.pending,
          version: latestVersion,
        });
      }}
    />
  ) : undefined;

  return (
    <DynamicConnectionForm
      connectionType={currentConnectionType}
      initValues={connection.data}
      isLoading={mutation.isPending || connection.isFetching}
      onSubmit={handleSubmit}
      mode="edit"
      onDelete={handleDelete}
      isDeleting={deleteMutation.isPending}
      rateLimitStatus={rateLimitStatusQuery.data?.data}
      showReconnect={isOAuthAuthCode}
      onReconnect={id ? () => authorize(id) : undefined}
      isReconnecting={isAuthorizing(id)}
      needsReconnect={needsReconnect}
      conflictNotice={conflictNotice}
    />
  );
}
