import { useCallback, useEffect, useRef, useState } from 'react';
import { useAuth } from 'react-oidc-context';
import { getRuntimeBaseUrl } from '@/shared/queries/utils';
import { readJsonEventStream } from '@/shared/utils/sse';
import {
  snapshotStuckAfterMs,
  stickyChokepoint,
  type PipelineSnapshot,
} from '../utils/pipeline';

/// How many snapshots to keep for the sparklines.
///
/// The sampler ticks once a second, so this is a minute of history — long
/// enough to show whether a full stage is recycling or holding, which is the
/// distinction the whole view exists to make.
export const HISTORY_LENGTH = 60;

export interface PipelineStreamState {
  snapshot: PipelineSnapshot | null;
  /// Per-stage occupancy history, oldest first, `null` where a reading was
  /// missing. Gaps are preserved rather than interpolated: a line drawn through
  /// an outage claims knowledge of a period nothing was observed.
  history: Record<string, (number | null)[]>;
  /// The stage currently held as the constraint, damped across ticks.
  chokepointKey: string | null;
  connected: boolean;
  error: string | null;
}

/// Subscribe to live pipeline snapshots, falling back to polling.
///
/// The stream is the normal path; polling exists because a proxy that buffers
/// `text/event-stream` turns a live view into a page that never updates, and
/// silently showing stale numbers is worse than showing slower ones.
export function usePipelineStream(): PipelineStreamState {
  const auth = useAuth();
  const token = auth.user?.access_token;

  const [snapshot, setSnapshot] = useState<PipelineSnapshot | null>(null);
  const [history, setHistory] = useState<Record<string, (number | null)[]>>({});
  const [chokepointKey, setChokepointKey] = useState<string | null>(null);
  const [connected, setConnected] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Read inside the state updater rather than as a dependency, so accepting a
  // snapshot does not re-create the effect and tear down the stream.
  const chokepointRef = useRef<string | null>(null);

  const accept = useCallback((next: PipelineSnapshot) => {
    setSnapshot(next);
    setHistory((prev) => {
      const updated: Record<string, (number | null)[]> = {};
      for (const stage of next.stages) {
        const series = [...(prev[stage.key] ?? []), stage.used];
        updated[stage.key] = series.slice(-HISTORY_LENGTH);
      }
      return updated;
    });
    const held = stickyChokepoint(
      next.stages,
      chokepointRef.current,
      snapshotStuckAfterMs(next)
    );
    chokepointRef.current = held;
    setChokepointKey(held);
  }, []);

  useEffect(() => {
    const controller = new AbortController();
    let pollTimer: ReturnType<typeof setInterval> | undefined;
    let cancelled = false;

    const headers: Record<string, string> = { Accept: 'text/event-stream' };
    if (token) headers.Authorization = `Bearer ${token}`;

    async function poll() {
      try {
        const response = await fetch(
          `${getRuntimeBaseUrl()}/analytics/pipeline`,
          {
            headers: token ? { Authorization: `Bearer ${token}` } : {},
            signal: controller.signal,
          }
        );
        // 503 means the sampler has not produced a snapshot yet, which is a
        // booting server rather than a failure. Keep waiting quietly.
        if (response.status === 503) return;
        if (!response.ok) throw new Error(`HTTP ${response.status}`);
        const body = await response.json();
        if (!cancelled && body?.data) accept(body.data as PipelineSnapshot);
      } catch (e) {
        if (!cancelled && !controller.signal.aborted) {
          setError(e instanceof Error ? e.message : 'Failed to read pipeline');
        }
      }
    }

    function startPolling() {
      if (pollTimer || cancelled) return;
      setConnected(false);
      void poll();
      pollTimer = setInterval(() => void poll(), 2000);
    }

    async function stream() {
      try {
        const response = await fetch(
          `${getRuntimeBaseUrl()}/analytics/pipeline/stream`,
          { headers, signal: controller.signal }
        );
        if (!response.ok || !response.body) {
          throw new Error(`HTTP ${response.status}`);
        }

        setConnected(true);
        setError(null);

        for await (const value of readJsonEventStream<PipelineSnapshot>(
          response.body,
          controller.signal
        )) {
          if (cancelled) break;
          accept(value);
        }

        // The stream ended without an error — a redeploy, or a proxy timing
        // out an idle connection. Polling keeps the page truthful until the
        // component remounts.
        if (!cancelled) startPolling();
      } catch (e) {
        if (cancelled || controller.signal.aborted) return;
        setError(e instanceof Error ? e.message : 'Pipeline stream failed');
        startPolling();
      }
    }

    void stream();

    return () => {
      cancelled = true;
      controller.abort();
      if (pollTimer) clearInterval(pollTimer);
    };
  }, [token, accept]);

  return { snapshot, history, chokepointKey, connected, error };
}
