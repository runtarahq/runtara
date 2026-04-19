import {
  ChatSSEEvent,
  ChatSSEEventType,
} from '@/features/workflows/types/chat';

/**
 * Parse a single SSE text block into a ChatSSEEvent.
 * A block looks like:
 *   event: started\n
 *   data: {"type":"started","instance_id":"uuid"}\n
 */
export function parseSingleSSEBlock(block: string): ChatSSEEvent | null {
  const lines = block.split('\n');
  let eventType: string | undefined;
  const dataLines: string[] = [];

  for (const line of lines) {
    if (line.startsWith('event:')) {
      eventType = line.slice('event:'.length).trim();
    } else if (line.startsWith('data:')) {
      dataLines.push(line.slice('data:'.length).trim());
    }
  }

  if (dataLines.length === 0) {
    return null;
  }

  const rawData = dataLines.join('\n');

  let data: Record<string, unknown>;
  try {
    data = JSON.parse(rawData);
  } catch {
    // If data isn't valid JSON, wrap it as a message
    data = { raw: rawData };
  }

  // Use event field if present, otherwise fall back to data.type, then 'message'
  const type = (eventType || data.type || 'message') as ChatSSEEventType;

  return {
    type,
    data,
    timestamp: new Date().toISOString(),
  };
}

/**
 * Async generator that consumes a ReadableStream<Uint8Array> from fetch()
 * and yields parsed SSE events.
 */
export async function* parseSSEStream(
  stream: ReadableStream<Uint8Array>,
  signal?: AbortSignal
): AsyncGenerator<ChatSSEEvent> {
  const reader = stream.getReader();
  const decoder = new TextDecoder();
  let buffer = '';

  try {
    while (true) {
      if (signal?.aborted) {
        break;
      }

      const { done, value } = await reader.read();
      if (done) break;

      const chunk = decoder.decode(value, { stream: true });
      console.debug(
        '[SSE] chunk received:',
        JSON.stringify(chunk).slice(0, 200)
      );
      buffer += chunk;

      // Normalize \r\n to \n so splitting works regardless of server line endings
      buffer = buffer.replace(/\r\n/g, '\n').replace(/\r/g, '\n');

      // SSE messages are separated by double newlines
      const parts = buffer.split('\n\n');

      // Last part may be incomplete — keep it in buffer
      buffer = parts.pop() || '';

      for (const part of parts) {
        const trimmed = part.trim();
        if (!trimmed) continue;

        const event = parseSingleSSEBlock(trimmed);
        if (event) {
          console.debug('[SSE] yielding event:', event.type);
          yield event;
        }
      }
    }

    // Process any remaining buffer
    console.debug(
      '[SSE] stream done, remaining buffer:',
      JSON.stringify(buffer).slice(0, 200)
    );
    if (buffer.trim()) {
      const event = parseSingleSSEBlock(buffer.trim());
      if (event) {
        console.debug('[SSE] yielding final event:', event.type);
        yield event;
      }
    }
  } finally {
    reader.releaseLock();
  }
}
