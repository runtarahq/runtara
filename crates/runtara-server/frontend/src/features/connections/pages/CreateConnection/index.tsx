import { useMemo, useCallback } from 'react';
import { flushSync } from 'react-dom';
import { useNavigate, useParams } from 'react-router';
import { toast } from 'sonner';
import { useCustomMutation, useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { ConnectionTypeDto } from '@/generated/RuntaraRuntimeApi';
import { Loader2 } from '@/shared/components/loader.tsx';
import { DynamicConnectionForm } from '@/features/connections/components/Forms/DynamicConnectionForm';
import {
  createConnection,
  getConnectionTypes,
  getOAuthAuthorizeUrl,
} from '@/features/connections/queries';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import {
  useOAuthPopup,
  OAuthPopupClosedError,
} from '@/features/connections/hooks/useOAuthPopup';
import { useAuth } from 'react-oidc-context';
import { isOidcAuth } from '@/shared/config/runtimeConfig';
import { useNavigationBlockerStore } from '@/shared/stores/navigationBlockerStore';
import { queryClient } from '@/main.tsx';

export function CreateConnection() {
  const { id } = useParams();

  const navigate = useNavigate();
  const { openOAuthPopup } = useOAuthPopup();
  const auth = useAuth();
  const token = auth.user?.access_token;

  const { data: connectionTypes, isFetching } = useCustomQuery({
    queryKey: queryKeys.connections.types(),
    queryFn: getConnectionTypes,
  });

  const isOAuthType = useMemo(() => {
    const ct = (connectionTypes ?? []).find(
      (ct: ConnectionTypeDto) => ct.integrationId === id
    );
    return !!(ct as unknown as Record<string, unknown>)?.oauthConfig;
  }, [id, connectionTypes]);

  const startOAuthFlow = useCallback(
    async (connectionId: string) => {
      // Only OIDC mode needs a bearer token; local / trust_proxy modes have
      // none and the server accepts unauthenticated calls there.
      if (isOidcAuth && !token) {
        navigate('/connections');
        return;
      }
      try {
        const authUrl = await getOAuthAuthorizeUrl(token, connectionId);
        await openOAuthPopup(authUrl);
        // Authorized: nothing left to do here, the list confirms completion.
        queryClient.invalidateQueries({ queryKey: queryKeys.connections.all });
        navigate('/connections');
        toast.success('Connection authorized and ready.');
      } catch (error) {
        // The connection was created but not authorized. Land on its edit page
        // where the status card owns the recovery (Connect), rather than
        // stranding the user or dead-ending on the list.
        queryClient.invalidateQueries({ queryKey: queryKeys.connections.all });
        queryClient.invalidateQueries({
          queryKey: queryKeys.connections.byId(connectionId),
        });
        navigate(`/connections/${connectionId}`);
        if (error instanceof OAuthPopupClosedError) {
          toast.info(
            'Connection saved — authorization still needed. Finish connecting from this page.'
          );
        } else {
          toast.error(
            `Connection saved, but authorization didn't complete: ${
              error instanceof Error ? error.message : 'Unknown error'
            }. You can retry from this page.`
          );
        }
      }
    },
    [token, openOAuthPopup, navigate]
  );

  const { mutate, isPending } = useCustomMutation({
    mutationFn: createConnection,
    onSuccess: (connectionId: string) => {
      queryClient.invalidateQueries({ queryKey: queryKeys.connections.all });
      // The connection exists now: leaving must not trip the
      // unsaved-changes blocker. flushSync forces useBlocker to re-subscribe
      // with shouldBlock=false before the navigate below runs.
      flushSync(() => {
        useNavigationBlockerStore.getState().setBlocker(false);
      });
      if (isOAuthType && connectionId) {
        startOAuthFlow(connectionId);
      } else {
        navigate('/connections');
        toast.success('Connection created.');
      }
    },
  });

  const currentConnectionType = useMemo(() => {
    return (
      (connectionTypes ?? []).find(
        (ct: ConnectionTypeDto) => ct.integrationId === id
      ) || null
    );
  }, [id, connectionTypes]);

  // Set page title
  usePageTitle('Create Connection');

  const handleSubmit = (data: Record<string, unknown>) => {
    const {
      title,
      rateLimitEnabled,
      requestsPerSecond,
      burstSize,
      maxRetries,
      maxWaitMs,
      retryOnLimit,
      isDefaultFileStorage,
      defaultFor,
      ...parameters
    } = data;

    const rateLimitConfig = rateLimitEnabled
      ? {
          requestsPerSecond: Number(requestsPerSecond),
          burstSize: Number(burstSize),
          maxRetries: Number(maxRetries),
          maxWaitMs: Number(maxWaitMs),
          retryOnLimit: Boolean(retryOnLimit),
        }
      : undefined;

    mutate({
      title: title as string,
      connectionParameters: parameters,
      integrationId: currentConnectionType?.integrationId,
      rateLimitConfig,
      isDefaultFileStorage: isDefaultFileStorage
        ? Boolean(isDefaultFileStorage)
        : undefined,
      defaultFor: Array.isArray(defaultFor) ? (defaultFor as string[]) : [],
    });
  };

  if (isFetching) {
    return (
      <div className="mx-auto max-w-5xl px-4 py-10 sm:px-6 lg:px-10">
        <Loader2 />
      </div>
    );
  }

  if (!currentConnectionType) {
    return (
      <div className="mx-auto max-w-5xl px-4 py-10 sm:px-6 lg:px-10">
        <div className="rounded-lg bg-muted/20 px-6 py-8 text-center text-muted-foreground">
          Connection type not found.
        </div>
      </div>
    );
  }

  return (
    <DynamicConnectionForm
      connectionType={currentConnectionType}
      isLoading={isPending}
      onSubmit={handleSubmit}
      mode="create"
      oauthCreateHint={isOAuthType}
    />
  );
}
