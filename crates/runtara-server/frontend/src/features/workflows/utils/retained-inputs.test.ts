import { describe, expect, it, vi } from 'vitest';
import { InputSubmissionError } from './input-submission';
import { RetainedInputs, type InputIntentRequest } from './retained-inputs';

const request = (): InputIntentRequest => ({
  kind: 'report',
  reportId: 'report',
  blockId: 'approve',
  instanceId: 'instance',
  requestId: 'wait',
  payload: { answer: true },
  filters: { region: 'one' },
  blockFilters: {},
});
const receipt = {
  receiptId: 'receipt',
  requestId: 'wait',
  acceptedAt: '2026-09-25T00:00:00Z',
};

describe('retained input intents', () => {
  it('freezes caller context and retains old uncertain operations after edits', async () => {
    const store = new RetainedInputs();
    const original = request();
    const first = store.prepare(original, 'Approval');
    await store.send(first.operationId, async () => {
      throw new Error('Acknowledgement lost');
    });
    original.payload.answer = false;
    if (original.kind === 'report') original.filters.region = 'two';
    const second = store.prepare(original, 'Approval');
    expect(second.operationId).not.toBe(first.operationId);
    expect(store.prepare(request(), 'renamed label').operationId).toBe(
      first.operationId
    );
    const send = vi.fn().mockResolvedValue(receipt);
    expect(await store.send(first.operationId, send)).toBe(true);
    expect(send.mock.calls[0][0].request).toEqual(request());
    expect(store.snapshot().map((input) => input.state)).toEqual([
      'accepted',
      'uncertain',
    ]);
  });

  it('does not confuse instances with the same request hash or different report blocks', () => {
    const store = new RetainedInputs();
    const one = request();
    const two = { ...one, instanceId: 'second' };
    const three = { ...request(), blockId: 'other' } as InputIntentRequest;
    expect(
      new Set(
        [one, two, three].map(
          (value) => store.prepare(value, 'Approval').operationId
        )
      ).size
    ).toBe(3);
  });

  it('prevents duplicate concurrent sends and isolates out-of-order outcomes', async () => {
    const store = new RetainedInputs();
    const first = store.prepare(request(), 'First');
    const second = store.prepare(
      { ...request(), payload: { answer: false } },
      'Second'
    );
    let resolve!: (value: unknown) => void;
    const send = vi.fn(
      () =>
        new Promise((done) => {
          resolve = done;
        })
    );
    const pending = store.send(first.operationId, send);
    expect(await store.send(first.operationId, send)).toBe(false);
    expect(send).toHaveBeenCalledTimes(1);
    await store.send(second.operationId, async () => {
      throw new InputSubmissionError(
        'Conflict',
        'INPUT_OPERATION_CONFLICT',
        409
      );
    });
    resolve(receipt);
    await pending;
    expect(store.snapshot().map((input) => input.state)).toEqual([
      'accepted',
      'rejected',
    ]);
  });

  it.each([
    undefined,
    {},
    { ...receipt, requestId: 'other' },
    { ...receipt, receiptId: '' },
  ])(
    'keeps the operation uncertain for an invalid success receipt %j',
    async (response) => {
      const store = new RetainedInputs();
      const first = store.prepare(request(), 'Approval');
      expect(await store.send(first.operationId, async () => response)).toBe(
        false
      );
      expect(store.snapshot()[0].state).toBe('uncertain');
      expect(store.prepare(request(), 'Approval').operationId).toBe(
        first.operationId
      );
    }
  );
});
