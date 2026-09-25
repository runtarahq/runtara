import { afterEach, describe, expect, it, vi } from 'vitest';
import { deliverSignal, getPendingInput } from './index';
import { checkPendingInput, sendSessionMessage } from './chat';
import { InputSubmissionError } from '../utils/input-submission';

vi.mock('@/shared/queries', () => ({ RuntimeREST: { api: {} } }));
const submission = {
  requestId: 'request',
  operationId: 'operation',
  payload: { answer: true },
};
const receipt = {
  requestId: 'request',
  receiptId: 'receipt',
  acceptedAt: 'now',
};
afterEach(() => vi.unstubAllGlobals());

describe('managed input HTTP contract', () => {
  it('sends stable queue identities and rejects a mismatched enqueue acknowledgement', async () => {
    const submission = {
      messageId: 'message',
      operationId: 'operation',
      message: 'Hello',
    };
    const data = { ...submission, state: 'queued', enqueuedAtMs: 1 };
    const fetch = vi
      .fn()
      .mockImplementation(
        async () => new Response(JSON.stringify({ success: true, data }))
      );
    vi.stubGlobal('fetch', fetch);
    expect(await sendSessionMessage('token', 'session', submission)).toEqual(
      data
    );
    expect(JSON.parse(fetch.mock.calls[0][1].body)).toEqual(submission);
    fetch.mockResolvedValueOnce(
      new Response(
        JSON.stringify({
          success: true,
          data: { ...data, operationId: 'wrong' },
        })
      )
    );
    await expect(
      sendSessionMessage('token', 'session', submission)
    ).rejects.toThrow('could not be confirmed');
  });
  it('sends stable request and operation IDs and returns only a confirmed receipt', async () => {
    const fetch = vi
      .fn()
      .mockResolvedValue(
        new Response(JSON.stringify({ success: true, data: receipt }))
      );
    vi.stubGlobal('fetch', fetch);
    expect(await deliverSignal('token', 'instance', submission)).toEqual(
      receipt
    );
    expect(JSON.parse(fetch.mock.calls[0][1].body)).toEqual(submission);
    expect(fetch.mock.calls[0][1].body).not.toContain('checkpointId');
  });
  it('preserves typed stale conflicts', async () => {
    vi.stubGlobal(
      'fetch',
      vi
        .fn()
        .mockResolvedValue(
          new Response(
            JSON.stringify({ code: 'INPUT_INACTIVE', message: 'Closed' }),
            { status: 409 }
          )
        )
    );
    await expect(
      deliverSignal('token', 'instance', submission)
    ).rejects.toMatchObject({ code: 'INPUT_INACTIVE', status: 409 });
  });
  it('does not acknowledge a malformed or mismatched success response', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(
        new Response(
          JSON.stringify({
            success: true,
            data: { ...receipt, requestId: 'different' },
          })
        )
      )
    );
    await expect(
      deliverSignal('token', 'instance', submission)
    ).rejects.toBeInstanceOf(InputSubmissionError);
  });
  it('unwraps the same authoritative pending page for session and execution callers', async () => {
    const data = {
      instanceId: 'instance',
      pendingInputs: [{ requestId: 'request' }],
      count: 1,
    };
    vi.stubGlobal(
      'fetch',
      vi
        .fn()
        .mockImplementation(
          async () => new Response(JSON.stringify({ success: true, data }))
        )
    );
    expect(await checkPendingInput('token', 'session')).toEqual(data);
    expect(await getPendingInput('token', 'workflow', 'instance')).toEqual(
      data.pendingInputs
    );
  });
  it('does not interpret malformed discovery as an empty list', async () => {
    vi.stubGlobal(
      'fetch',
      vi
        .fn()
        .mockImplementation(
          async () => new Response(JSON.stringify({ success: false }))
        )
    );
    await expect(checkPendingInput('token', 'session')).rejects.toThrow(
      'Invalid pending input response'
    );
    await expect(
      getPendingInput('token', 'workflow', 'instance')
    ).rejects.toThrow('Invalid pending input response');
  });
});
