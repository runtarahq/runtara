import { describe, it, expect } from 'vitest';
import { parseDataFrame, readJsonEventStream } from './sse';

function streamOf(...chunks: string[]): ReadableStream<Uint8Array> {
  const encoder = new TextEncoder();
  return new ReadableStream({
    start(controller) {
      for (const chunk of chunks) controller.enqueue(encoder.encode(chunk));
      controller.close();
    },
  });
}

describe('parseDataFrame', () => {
  it('parses a single data line', () => {
    expect(parseDataFrame('data: {"a":1}')).toEqual({ a: 1 });
  });

  it('joins multi-line data as one document', () => {
    expect(parseDataFrame('data: {"a":\ndata: 1}')).toEqual({ a: 1 });
  });

  it('ignores the event name, which this reader has no use for', () => {
    expect(parseDataFrame('event: snapshot\ndata: {"a":1}')).toEqual({ a: 1 });
  });

  it('skips a keep-alive comment rather than treating it as data', () => {
    // The server sends these to hold the connection open through proxies. A
    // reader that choked on them would drop the stream on an idle system.
    expect(parseDataFrame(': keep-alive')).toBeUndefined();
    expect(parseDataFrame('')).toBeUndefined();
  });

  it('skips a malformed frame instead of throwing', () => {
    // One bad frame must not end a stream the page depends on.
    expect(parseDataFrame('data: {not json')).toBeUndefined();
  });
});

describe('readJsonEventStream', () => {
  it('yields every complete frame', async () => {
    const stream = streamOf('data: {"n":1}\n\n', 'data: {"n":2}\n\n');
    const seen: unknown[] = [];
    for await (const value of readJsonEventStream(stream)) seen.push(value);
    expect(seen).toEqual([{ n: 1 }, { n: 2 }]);
  });

  it('reassembles a frame split across chunks', () => {
    // A snapshot with six stages will not arrive in one TCP read, so a reader
    // that assumed frame boundaries matched chunk boundaries would drop most
    // of them.
    const stream = streamOf('data: {"n":', '1}\n\ndata: {"n":2}\n\n');
    return (async () => {
      const seen: unknown[] = [];
      for await (const value of readJsonEventStream(stream)) seen.push(value);
      expect(seen).toEqual([{ n: 1 }, { n: 2 }]);
    })();
  });

  it('handles CRLF line endings', async () => {
    const stream = streamOf('data: {"n":1}\r\n\r\n');
    const seen: unknown[] = [];
    for await (const value of readJsonEventStream(stream)) seen.push(value);
    expect(seen).toEqual([{ n: 1 }]);
  });

  it('yields a final frame that arrives without a trailing blank line', async () => {
    const stream = streamOf('data: {"n":1}');
    const seen: unknown[] = [];
    for await (const value of readJsonEventStream(stream)) seen.push(value);
    expect(seen).toEqual([{ n: 1 }]);
  });

  it('carries on past a malformed frame', async () => {
    const stream = streamOf('data: broken\n\ndata: {"n":2}\n\n');
    const seen: unknown[] = [];
    for await (const value of readJsonEventStream(stream)) seen.push(value);
    expect(seen).toEqual([{ n: 2 }]);
  });

  it('stops when the caller aborts', async () => {
    const controller = new AbortController();
    controller.abort();
    const stream = streamOf('data: {"n":1}\n\n');
    const seen: unknown[] = [];
    for await (const value of readJsonEventStream(stream, controller.signal)) {
      seen.push(value);
    }
    expect(seen).toEqual([]);
  });
});
