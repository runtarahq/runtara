import { useMemo, useCallback } from 'react';
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
import { useOAuthPopup } from '@/features/connections/hooks/useOAuthPopup';
import { useAuth } from 'react-oidc-context';
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
    initialData: [],
  });

  const isOAuthType = useMemo(() => {
    const ct = (connectionTypes ?? []).find(
      (ct: ConnectionTypeDto) => ct.integrationId === id
    );
    return !!(ct as unknown as Record<string, unknown>)?.oauthConfig;
  }, [id, connectionTypes]);

  const startOAuthFlow = useCallback(
    async (connectionId: string) => {
      if (!token) return;
      try {
        const authUrl = await getOAuthAuthorizeUrl(token, connectionId);
        await openOAuthPopup(authUrl);
        queryClient.invalidateQueries({ queryKey: queryKeys.connections.all });
        navigate('/connections');
        toast.success('Connection authorized successfully');
      } catch (error) {
        console.error('OAuth flow failed:', error);
        toast.error(
          `Authorization failed: ${error instanceof Error ? error.message : 'Unknown error'}. You can re-authorize from the connection page.`
        );
        queryClient.invalidateQueries({ queryKey: queryKeys.connections.all });
        navigate('/connections');
      }
    },
    [token, openOAuthPopup, navigate]
  );

  const { mutate, isPending } = useCustomMutation({
    mutationFn: createConnection,
    onSuccess: (connectionId: string) => {
      queryClient.invalidateQueries({ queryKey: queryKeys.connections.all });
      if (isOAuthType && connectionId) {
        startOAuthFlow(connectionId);
      } else {
        navigate('/connections');
        toast.info('Connection has been created');
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
        <div className="rounded-2xl bg-muted/20 px-6 py-8 text-center text-muted-foreground">
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
    />
  );
}
