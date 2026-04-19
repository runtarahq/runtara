import { describe, it, expect } from 'vitest';
import { parseSingleSSEBlock, parseSSEStream } from './sse';

describe('parseSingleSSEBlock', () => {
  it('parses a standard SSE block with event and data', () => {
    const block =
      'event: started\ndata: {"type":"started","instance_id":"abc-123"}';
    const result = parseSingleSSEBlock(block);

    expect(result).not.toBeNull();
    expect(result!.type).toBe('started');
    expect(result!.data.instance_id).toBe('abc-123');
  });

  it('falls back to data.type when event field is missing', () => {
    const block = 'data: {"type":"message","content":"Hello"}';
    const result = parseSingleSSEBlock(block);

    expect(result).not.toBeNull();
    expect(result!.type).toBe('message');
    expect(result!.data.content).toBe('Hello');
  });

  it('handles multi-line data fields', () => {
    const block = 'event: message\ndata: {"content":\ndata: "hello world"}';
    const result = parseSingleSSEBlock(block);

    expect(result).not.toBeNull();
    expect(result!.type).toBe('message');
    // Multi-line data is joined with newline; JSON should parse the concatenated result
    expect(result!.data.content).toBe('hello world');
  });

  it('handles malformed JSON gracefully', () => {
    const block = 'event: error\ndata: not valid json';
    const result = parseSingleSSEBlock(block);

    expect(result).not.toBeNull();
    expect(result!.type).toBe('error');
    expect(result!.data.raw).toBe('not valid json');
  });

  it('returns null for blocks with no data field', () => {
    const block = 'event: keepalive';
    const result = parseSingleSSEBlock(block);
    expect(result).toBeNull();
  });

  it('returns null for empty blocks', () => {
    const result = parseSingleSSEBlock('');
    expect(result).toBeNull();
  });

  it('defaults type to message when neither event nor data.type present', () => {
    const block = 'data: {"content":"just data"}';
    const result = parseSingleSSEBlock(block);

    expect(result).not.toBeNull();
    expect(result!.type).toBe('message');
  });
});

describe('parseSSEStream', () => {
  function createStream(chunks: string[]): ReadableStream<Uint8Array> {
    const encoder = new TextEncoder();
    let index = 0;
    return new ReadableStream({
      pull(controller) {
        if (index < chunks.length) {
          controller.enqueue(encoder.encode(chunks[index]));
          index++;
        } else {
          controller.close();
        }
      },
    });
  }

  async function collectEvents(stream: ReadableStream<Uint8Array>) {
    const events = [];
    for await (const event of parseSSEStream(stream)) {
      events.push(event);
    }
    return events;
  }

  it('parses a single complete SSE event', async () => {
    const stream = createStream([
      'event: started\ndata: {"type":"started","instance_id":"123"}\n\n',
    ]);

    const events = await collectEvents(stream);
    expect(events).toHaveLength(1);
    expect(events[0].type).toBe('started');
    expect(events[0].data.instance_id).toBe('123');
  });

  it('parses multiple SSE events in one chunk', async () => {
    const stream = createStream([
      'event: started\ndata: {"type":"started","instance_id":"123"}\n\n' +
        'event: message\ndata: {"content":"Hello"}\n\n',
    ]);

    const events = await collectEvents(stream);
    expect(events).toHaveLength(2);
    expect(events[0].type).toBe('started');
    expect(events[1].type).toBe('message');
  });

  it('handles events split across multiple chunks', async () => {
    const stream = createStream([
      'event: started\ndata: {"type":"st',
      'arted","instance_id":"123"}\n\n',
    ]);

    const events = await collectEvents(stream);
    expect(events).toHaveLength(1);
    expect(events[0].type).toBe('started');
    expect(events[0].data.instance_id).toBe('123');
  });

  it('handles multiple events across multiple chunks', async () => {
    const stream = createStream([
      'event: started\ndata: {"instance_id":"1"}\n\nevent: llm_',
      'start\ndata: {"iteration":1}\n\nevent: message\ndata: {"content":"Hi"}\n\n',
    ]);

    const events = await collectEvents(stream);
    expect(events).toHaveLength(3);
    expect(events[0].type).toBe('started');
    expect(events[1].type).toBe('llm_start');
    expect(events[2].type).toBe('message');
  });

  it('processes remaining buffer when stream ends', async () => {
    // Stream ends without a trailing \n\n
    const stream = createStream([
      'event: done\ndata: {"duration_seconds":4.2}',
    ]);

    const events = await collectEvents(stream);
    expect(events).toHaveLength(1);
    expect(events[0].type).toBe('done');
  });

  it('skips empty blocks between events', async () => {
    const stream = createStream([
      'event: started\ndata: {"instance_id":"1"}\n\n\n\nevent: done\ndata: {}\n\n',
    ]);

    const events = await collectEvents(stream);
    expect(events).toHaveLength(2);
  });

  it('handles \\r\\n line endings', async () => {
    const stream = createStream([
      'event: started\r\ndata: {"instance_id":"1"}\r\n\r\nevent: message\r\ndata: {"content":"Hi"}\r\n\r\n',
    ]);

    const events = await collectEvents(stream);
    expect(events).toHaveLength(2);
    expect(events[0].type).toBe('started');
    expect(events[1].type).toBe('message');
    expect(events[1].data.content).toBe('Hi');
  });

  it('handles mixed \\r\\n and \\n line endings', async () => {
    const stream = createStream([
      'event: started\r\ndata: {"instance_id":"1"}\n\n',
      'event: message\r\ndata: {"content":"Hello"}\r\n\r\n',
    ]);

    const events = await collectEvents(stream);
    expect(events).toHaveLength(2);
  });

  it('respects abort signal', async () => {
    const controller = new AbortController();
    const stream = createStream([
      'event: started\ndata: {"instance_id":"1"}\n\n',
      'event: message\ndata: {"content":"Hello"}\n\n',
    ]);

    const events = [];
    for await (const event of parseSSEStream(stream, controller.signal)) {
      events.push(event);
      controller.abort(); // Abort after first event
    }

    expect(events).toHaveLength(1);
  });
});
