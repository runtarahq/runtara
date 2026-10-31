import { useCallback, useRef } from 'react';
import { toast } from 'sonner';

interface OAuthResult {
  connectionId: string;
  success: boolean;
  error?: string;
}

/** The consent window was closed before the provider finished authorizing. */
export class OAuthPopupClosedError extends Error {
  constructor() {
    super('the authorization window was closed before it finished');
    this.name = 'OAuthPopupClosedError';
  }
}

// How often to check whether the popup was closed by the user.
const CLOSED_POLL_MS = 500;
// The callback posts its completion message around the time it closes the
// popup; wait this long after detecting a close before treating it as an
// abandon so a real success isn't misreported.
const CLOSED_GRACE_MS = 1500;
// How long to keep waiting once the popup looks closed. A provider page with
// Cross-Origin-Opener-Policy cuts the popup off from the console, and the
// browser then reports it closed while the user is still signing in; the
// landing page's broadcast still arrives when they finish. A closed window
// and a cut-off one look the same, so the user can cancel the wait.
const WAIT_AFTER_CLOSE_MS = 120000;
// Backstop for a popup that is neither completed nor closed (e.g. detached).
const TIMEOUT_MS = 300000;

// The console's OAuth landing page (public/oauth-complete.js) reports on this
// channel and to window.opener; see there.
const OAUTH_CHANNEL = 'runtara-oauth';

// The landing page reports failures as OAuth error codes.
const ERROR_MESSAGES: Record<string, string> = {
  access_denied: 'access was denied',
  invalid_state: 'the authorization expired or was already used',
  invalid_request: "the provider's answer was incomplete",
  token_exchange_failed: 'the provider did not issue a token',
};

function describeOAuthError(code: unknown): string {
  if (typeof code !== 'string' || !code) return 'OAuth authorization failed';
  return ERROR_MESSAGES[code] ?? `OAuth authorization failed (${code})`;
}

export function useOAuthPopup() {
  const cleanupRef = useRef<(() => void) | null>(null);

  const openOAuthPopup = useCallback(
    (
      authorizationUrl: string,
      /** The connection being authorized: a success reported for another
       *  one (another tab's popup, say) is not this popup's. */
      connectionId?: string
    ): Promise<OAuthResult> => {
      return new Promise((resolve, reject) => {
        // Stop listening for the previous popup, if any.
        cleanupRef.current?.();

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
        let waitTimer: ReturnType<typeof setTimeout> | undefined;
        let waitToast: string | number | undefined;
        const channel =
          typeof BroadcastChannel === 'undefined'
            ? null
            : new BroadcastChannel(OAUTH_CHANNEL);

        const cleanup = () => {
          window.removeEventListener('message', handler);
          channel?.close();
          cleanupRef.current = null;
          clearTimeout(timeoutId);
          clearInterval(closedPoll);
          if (closedGraceTimer) clearTimeout(closedGraceTimer);
          if (waitTimer) clearTimeout(waitTimer);
          if (waitToast !== undefined) toast.dismiss(waitToast);
        };

        const settle = (fn: () => void) => {
          if (settled) return;
          settled = true;
          cleanup();
          fn();
        };

        const onResult = (data: unknown) => {
          const result = data as Partial<OAuthResult> & { type?: string };
          if (result?.type !== 'oauth-complete') return;
          if (result.success) {
            if (connectionId && result.connectionId !== connectionId) return;
            settle(() =>
              resolve({
                connectionId: result.connectionId ?? connectionId ?? '',
                success: true,
              })
            );
          } else {
            settle(() => reject(new Error(describeOAuthError(result.error))));
          }
        };

        // The landing page is on the console's origin: anything else isn't it.
        const handler = (event: MessageEvent) => {
          if (event.origin !== window.location.origin) return;
          onResult(event.data);
        };

        cleanupRef.current = cleanup;
        window.addEventListener('message', handler);
        if (channel) channel.onmessage = (event) => onResult(event.data);

        const abandon = () => settle(() => reject(new OAuthPopupClosedError()));

        // Notice a closed consent window instead of hanging for the full
        // timeout. Give the completion message a short grace window in case it
        // races the close; after that, keep listening for a while (see
        // WAIT_AFTER_CLOSE_MS) unless only the opener could bring the result.
        const closedPoll = setInterval(() => {
          if (settled || !popup.closed || closedGraceTimer) return;
          closedGraceTimer = setTimeout(() => {
            if (!channel) {
              abandon();
              return;
            }
            waitToast = toast.loading('Waiting for the sign-in to finish', {
              description:
                'The sign-in window closed or lost contact. If you are still signing in, finish there.',
              duration: Infinity,
              action: { label: 'Cancel', onClick: abandon },
            });
            waitTimer = setTimeout(abandon, WAIT_AFTER_CLOSE_MS);
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
