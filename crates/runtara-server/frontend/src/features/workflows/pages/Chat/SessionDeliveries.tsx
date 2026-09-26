import { useEffect, useRef, useState } from 'react';
import type {
  DeliveryStatus,
  ResolveDeliveryRequest,
} from '@/generated/RuntaraRuntimeApi';
import { Button } from '@/shared/components/ui/button';
import {
  listSessionDeliveries,
  resolveSessionDelivery,
} from '../../queries/chat';
import { useChatStore } from '../../stores/chatStore';

const reasons: Record<string, string> = {
  no_target: 'Waiting for an input request',
  ambiguous_target: 'Choose which request should receive this message',
  stale_target: 'The original request is no longer active',
  invalid_payload: 'The message does not match the requested response format',
  operation_conflict: 'This message conflicts with an earlier response',
  backend_unavailable: 'Delivery is temporarily unavailable',
  explicit_failure: 'Delivery was explicitly stopped',
  launch_rejected: 'The next execution could not be started',
};

/** Delivery recovery and visibility do not depend on the SSE subscriber. */
export function SessionDeliveries({ token }: { token: string }) {
  const sessionId = useChatStore((state) => state.sessionId);
  const instanceId = useChatStore((state) => state.instanceId);
  const pending = useChatStore((state) => state.pendingInputs);
  const [snapshot, setSnapshot] = useState<{
    sessionId: string;
    rows: DeliveryStatus[];
  }>();
  const [error, setError] = useState<string | null>(null);
  const [resolutionError, setResolutionError] = useState<{
    sessionId: string;
    message: string;
  } | null>(null);
  const [resolving, setResolving] = useState(false);
  const resolvingRef = useRef(false);
  const generation = useRef(0);
  const [refresh, setRefresh] = useState(0);

  useEffect(() => {
    if (!sessionId) return;
    const controller = new AbortController();
    const version = ++generation.current;
    let cursor = '0';
    let delay = 1000;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let scanned = new Map<string, DeliveryStatus>();
    const current = () =>
      !controller.signal.aborted &&
      generation.current === version &&
      useChatStore.getState().sessionId === sessionId;
    setError(null);
    const poll = async () => {
      try {
        const page = await listSessionDeliveries(
          token,
          sessionId,
          cursor,
          controller.signal
        );
        if (!current()) return;
        for (const row of page.deliveries) scanned.set(row.messageId, row);
        cursor = page.nextCursor;
        const completed = cursor === '0';
        const collected = new Map(scanned);
        setSnapshot((previous) => {
          const rows =
            completed || previous?.sessionId !== sessionId
              ? collected
              : new Map(previous.rows.map((row) => [row.messageId, row]));
          for (const [id, row] of collected) rows.set(id, row);
          return {
            sessionId,
            rows: [...rows.values()].sort(
              (a, b) => b.enqueuedAtMs - a.enqueuedAtMs
            ),
          };
        });
        if (completed) scanned = new Map();
        setError(null);
        delay = completed ? 3000 : 1000;
      } catch (error) {
        if (!current()) return;
        setError(
          error instanceof Error ? error.message : 'Delivery status unavailable'
        );
        delay = Math.min(delay * 2, 30000);
      }
      if (current()) timer = setTimeout(poll, delay);
    };
    void poll();
    return () => {
      controller.abort();
      clearTimeout(timer);
    };
  }, [sessionId, token, refresh]);

  const resolve = async (
    messageId: string,
    request: ResolveDeliveryRequest
  ) => {
    if (!sessionId || resolvingRef.current) return;
    resolvingRef.current = true;
    setResolving(true);
    const version = ++generation.current; // Discard snapshots read before the mutation.
    try {
      const result = await resolveSessionDelivery(
        token,
        sessionId,
        messageId,
        request
      );
      if (
        useChatStore.getState().sessionId !== sessionId ||
        version !== generation.current
      )
        return;
      setSnapshot((previous) =>
        previous?.sessionId === sessionId
          ? {
              ...previous,
              rows: previous.rows.map((row) =>
                row.messageId === messageId ? result : row
              ),
            }
          : previous
      );
      setError(null);
      setResolutionError(null);
    } catch (error) {
      if (
        useChatStore.getState().sessionId === sessionId &&
        version === generation.current
      )
        setResolutionError({
          sessionId,
          message:
            error instanceof Error
              ? error.message
              : 'Delivery resolution unavailable',
        });
    } finally {
      resolvingRef.current = false;
      setResolving(false);
      setRefresh((value) => value + 1);
    }
  };

  if (!sessionId) return null;
  const rows = snapshot?.sessionId === sessionId ? snapshot.rows : [];
  const visibleError =
    resolutionError?.sessionId === sessionId ? resolutionError.message : error;
  if (!rows.length && !visibleError) return null;
  return (
    <details
      className="max-h-64 overflow-auto border-t px-4 py-2"
      open={!!visibleError || rows.some((row) => row.state === 'blocked')}
    >
      <summary className="text-sm">
        Message delivery ({rows.length} loaded)
      </summary>
      {visibleError && (
        <p role="alert" className="text-sm text-destructive">
          {visibleError}
        </p>
      )}
      <ul className="space-y-3 py-2">
        {rows.map((row) => (
          <li key={row.messageId} className="space-y-1 text-sm">
            <p>
              Message {row.messageId.slice(0, 8)}:{' '}
              {row.state === 'accepted' ? 'Accepted by workflow' : row.state}
            </p>
            {row.reason && (
              <p className="text-muted-foreground">
                {reasons[row.reason] ?? row.reason}
              </p>
            )}
            {row.state === 'blocked' && (
              <div className="flex flex-wrap gap-2">
                {row.instanceId && row.requestId ? (
                  <Button
                    size="sm"
                    variant="secondary"
                    disabled={resolving}
                    onClick={() =>
                      void resolve(row.messageId, {
                        action: 'select',
                        instanceId: row.instanceId!,
                        requestId: row.requestId!,
                      })
                    }
                  >
                    Retry original request
                  </Button>
                ) : (
                  pending
                    .filter(
                      (request) =>
                        (request.instanceId ?? instanceId) === instanceId
                    )
                    .map((request) => (
                      <Button
                        key={request.requestId}
                        size="sm"
                        variant="secondary"
                        disabled={resolving || !instanceId}
                        onClick={() => {
                          if (instanceId)
                            void resolve(row.messageId, {
                              action: 'select',
                              instanceId,
                              requestId: request.requestId,
                            });
                        }}
                      >
                        Send to{' '}
                        {request.message ||
                          request.toolName ||
                          request.requestId.slice(0, 8)}
                      </Button>
                    ))
                )}
                <Button
                  size="sm"
                  variant="secondary"
                  disabled={resolving}
                  onClick={() =>
                    void resolve(row.messageId, { action: 'fail' })
                  }
                >
                  Stop this delivery
                </Button>
              </div>
            )}
          </li>
        ))}
      </ul>
    </details>
  );
}
