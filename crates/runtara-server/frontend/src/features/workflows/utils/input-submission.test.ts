import { describe, expect, it } from 'vitest';
import { InputSubmissionTracker } from './input-submission';

describe('managed input submissions', () => {
  it('reuses an uncertain operation across equivalent payloads and isolates edits', () => {
    const tracker = new InputSubmissionTracker();
    const payload = { answer: { b: 2, a: 1 }, items: [1, 2] };
    const first = tracker.prepare('instance', 'request', payload);
    payload.items.push(3);
    expect(first.payload.items).toEqual([1, 2]);
    expect(
      tracker.prepare('instance', 'request', {
        items: [1, 2],
        answer: { a: 1, b: 2 },
      }).operationId
    ).toBe(first.operationId);
    expect(
      tracker.prepare('instance', 'request', payload).operationId
    ).not.toBe(first.operationId);
  });

  it('never reuses an operation for another request or instance', () => {
    const tracker = new InputSubmissionTracker();
    const ids = [
      tracker.prepare('one', 'wait', {}).operationId,
      tracker.prepare('two', 'wait', {}).operationId,
      tracker.prepare('one', 'next', {}).operationId,
    ];
    expect(new Set(ids).size).toBe(3);
  });
});
