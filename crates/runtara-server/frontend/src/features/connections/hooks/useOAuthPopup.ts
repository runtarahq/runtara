import { useCallback, useRef } from 'react';

interface OAuthResult {
  connectionId: string;
  success: boolean;
  error?: string;
}

/** The consent window was closed before the provider finished authorizing. */
export class OAuthPopupClosedError extends Error {
  constructor() {
    super('The authorization window was closed before it finished.');
    this.name = 'OAuthPopupClosedError';
  }
}

// How often to check whether the popup was closed by the user.
const CLOSED_POLL_MS = 500;
// The callback posts its completion message around the time it closes the
// popup; wait this long after detecting a close before treating it as an
// abandon so a real success isn't misreported.
const CLOSED_GRACE_MS = 1500;
// Backstop for a popup that is neither completed nor closed (e.g. detached).
const TIMEOUT_MS = 300000;

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

        let settled = false;
        let closedGraceTimer: ReturnType<typeof setTimeout> | undefined;

        const cleanup = () => {
          window.removeEventListener('message', handler);
          listenerRef.current = null;
          clearTimeout(timeoutId);
          clearInterval(closedPoll);
          if (closedGraceTimer) clearTimeout(closedGraceTimer);
        };

        const settle = (fn: () => void) => {
          if (settled) return;
          settled = true;
          cleanup();
          fn();
        };

        const handler = (event: MessageEvent) => {
          if (event.origin !== window.location.origin) return;
          if (event.data?.type !== 'oauth-complete') return;
          if (event.data.success) {
            settle(() =>
              resolve({ connectionId: event.data.connectionId, success: true })
            );
          } else {
            settle(() =>
              reject(new Error(event.data.error || 'OAuth authorization failed'))
            );
          }
        };

        listenerRef.current = handler;
        window.addEventListener('message', handler);

        // Detect an abandoned consent window promptly instead of hanging for
        // the full timeout. Give the completion message a short grace window
        // in case it races the close.
        const closedPoll = setInterval(() => {
          if (settled || !popup.closed || closedGraceTimer) return;
          closedGraceTimer = setTimeout(() => {
            settle(() => reject(new OAuthPopupClosedError()));
          }, CLOSED_GRACE_MS);
        }, CLOSED_POLL_MS);

        // Backstop: 5-minute timeout for a popup that never completes or closes.
        const timeoutId = setTimeout(() => {
          if (!popup.closed) popup.close();
          settle(() => reject(new Error('OAuth authorization timed out')));
        }, TIMEOUT_MS);
      });
    },
    []
  );

  return { openOAuthPopup };
}
