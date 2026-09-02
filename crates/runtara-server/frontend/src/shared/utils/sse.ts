/// Read an SSE stream whose every message is a JSON document.
///
/// Deliberately separate from `features/workflows/utils/sse`, which infers a
/// chat event type from an `event:` line or a `type` field and logs each frame.
/// This one has no notion of event types and says nothing: it yields parsed
/// payloads. Generalising the chat reader instead would have meant changing a
/// working path to serve a simpler caller.

/// Consume a `fetch` body and yield each SSE `data:` payload, parsed.
///
/// Frames that are not valid JSON are skipped rather than thrown, because one
/// malformed frame must not end a stream a page depends on. Keep-alive
/// comments (`:` lines) carry no data and are skipped by the same rule.
export async function* readJsonEventStream<T>(
  stream: ReadableStream<Uint8Array>,
  signal?: AbortSignal
): AsyncGenerator<T> {
  const reader = stream.getReader();
  const decoder = new TextDecoder();
  let buffer = '';

  try {
    while (true) {
      if (signal?.aborted) break;

      const { done, value } = await reader.read();
      if (done) break;

      // Normalise line endings before splitting, so a server or proxy that
      // emits CRLF does not turn every frame into an unparseable fragment.
      buffer += decoder.decode(value, { stream: true });
      buffer = buffer.replace(/\r\n/g, '\n').replace(/\r/g, '\n');

      const frames = buffer.split('\n\n');
      // The trailing piece may be a partial frame; hold it for the next chunk.
      buffer = frames.pop() ?? '';

      for (const frame of frames) {
        const parsed = parseDataFrame<T>(frame);
        if (parsed !== undefined) yield parsed;
      }
    }

    const parsed = parseDataFrame<T>(buffer);
    if (parsed !== undefined) yield parsed;
  } finally {
    reader.releaseLock();
  }
}

/// Extract and parse the `data:` lines of one SSE frame.
///
/// Exported for tests. Returns `undefined` for a frame carrying no data or
/// data that is not JSON — both are ordinary in a live stream and neither is
/// worth interrupting the caller for.
export function parseDataFrame<T>(frame: string): T | undefined {
  const trimmed = frame.trim();
  if (!trimmed) return undefined;

  const data = trimmed
    .split('\n')
    .filter((line) => line.startsWith('data:'))
    .map((line) => line.slice('data:'.length).trim())
    .join('\n');

  if (!data) return undefined;

  try {
    return JSON.parse(data) as T;
  } catch {
    return undefined;
  }
}
