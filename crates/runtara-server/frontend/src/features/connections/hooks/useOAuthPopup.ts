import { useCallback, useRef } from 'react';

interface OAuthResult {
  connectionId: string;
  success: boolean;
  error?: string;
}

export function useOAuthPopup() {
  const listenerRef = useRef<((event: MessageEvent) => void) | null>(null);

  const openOAuthPopup = useCallback(
    (authorizationUrl: string): Promise<OAuthResult> => {
      return new Promise((resolve, reject) => {
        // Clean up any previous listener
        if (listenerRef.current) {
          window.removeEventListener('message', listenerRef.current);
        }

        const popup = window.open(
          authorizationUrl,
          'oauth-popup',
          'width=600,height=700,scrollbars=yes'
        );

        if (!popup) {
          reject(
            new Error(
              'Failed to open popup. Please allow popups for this site.'
            )
          );
          return;
        }

        const handler = (event: MessageEvent) => {
          if (event.origin !== window.location.origin) return;
          if (event.data?.type !== 'oauth-complete') return;

          window.removeEventListener('message', handler);
          listenerRef.current = null;
          clearTimeout(timeoutId);

          if (event.data.success) {
            resolve({
              connectionId: event.data.connectionId,
              success: true,
            });
          } else {
            reject(new Error(event.data.error || 'OAuth authorization failed'));
          }
        };

        listenerRef.current = handler;
        window.addEventListener('message', handler);

        // Timeout after 5 minutes
        const timeoutId = setTimeout(() => {
          window.removeEventListener('message', handler);
          listenerRef.current = null;
          if (!popup.closed) {
            popup.close();
          }
          reject(new Error('OAuth authorization timed out'));
        }, 300000);
      });
    },
    []
  );

  return { openOAuthPopup };
}
