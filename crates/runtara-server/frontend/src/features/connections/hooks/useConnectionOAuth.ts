import { useCallback, useState } from 'react';
import { useAuth } from 'react-oidc-context';
import { toast } from 'sonner';
import { isOidcAuth } from '@/shared/config/runtimeConfig';
import { queryKeys } from '@/shared/queries/query-keys';
import { queryClient } from '@/main.tsx';
import { getOAuthAuthorizeUrl } from '@/features/connections/queries';
import { useOAuthPopup } from './useOAuthPopup';

/**
 * Runs the interactive OAuth authorize popup for an EXISTING connection using
 * its already-stored credentials — no re-entry needed. Backs the "Reconnect"
 * affordance on the edit page and the connections list (and is the same flow
 * the create page auto-runs after saving an OAuth connection).
 *
 * `authorize` never rejects: it surfaces success/failure via toasts and
 * invalidates the connection queries so the status badge refreshes.
 */
export function useConnectionOAuth() {
  const auth = useAuth();
  const token = auth.user?.access_token;
  const { openOAuthPopup } = useOAuthPopup();
  const [runningId, setRunningId] = useState<string | null>(null);

  const authorize = useCallback(
    async (connectionId: string, opts?: { onSuccess?: () => void }) => {
      // Only OIDC mode needs a bearer token; local / trust_proxy modes have
      // none and the server accepts unauthenticated calls there.
      if (isOidcAuth && !token) return;
      setRunningId(connectionId);
      try {
        const authUrl = await getOAuthAuthorizeUrl(token, connectionId);
        await openOAuthPopup(authUrl);
        queryClient.invalidateQueries({ queryKey: queryKeys.connections.all });
        queryClient.invalidateQueries({
          queryKey: queryKeys.connections.byId(connectionId),
        });
        toast.success('Connection reconnected successfully');
        opts?.onSuccess?.();
      } catch (error) {
        toast.error(
          `Reconnect failed: ${
            error instanceof Error ? error.message : 'Unknown error'
          }. You can try again.`
        );
        // Refresh anyway — the server may still have flipped the status.
        queryClient.invalidateQueries({ queryKey: queryKeys.connections.all });
      } finally {
        setRunningId(null);
      }
    },
    [token, openOAuthPopup]
  );

  return {
    authorize,
    /** Whether an authorize popup is currently running for this id (or any). */
    isAuthorizing: (connectionId?: string) =>
      connectionId ? runningId === connectionId : runningId !== null,
  };
}
