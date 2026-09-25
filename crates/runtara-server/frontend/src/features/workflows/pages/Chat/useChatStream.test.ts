import { act, cleanup, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { useChatStream } from './useChatStream';
import { useChatStore } from '../../stores/chatStore';
import { sendSessionMessage } from '../../queries/chat';

const submitInput = vi.hoisted(() => vi.fn());
vi.mock('./useChatInputs', () => ({
  useChatInputs: () => ({ refreshPendingInput: vi.fn(), submitInput }),
}));
vi.mock('@/shared/hooks/useToken', () => ({ useToken: () => 'token' }));
vi.mock('../../queries/chat', () => ({
  sendSessionMessage: vi.fn(),
  createChatSession: vi.fn(),
  reconnectSession: vi.fn(),
}));
beforeEach(() => {
  vi.resetAllMocks();
  useChatStore.getState().resetChat();
  useChatStore.getState().resumeChat('workflow', 'Workflow', 'instance');
  useChatStore.getState().setSessionId('session');
});
afterEach(cleanup);

it('retries uncertain enqueue before newly discovered inputs and allocates fresh identities only for new intent', async () => {
  const { result } = renderHook(() => useChatStream('workflow'));
  vi.mocked(sendSessionMessage).mockRejectedValueOnce(
    new TypeError('lost acknowledgement')
  );
  await act(async () =>
    expect(await result.current.sendMessage('Hello')).toBe(false)
  );
  const original = vi.mocked(sendSessionMessage).mock.calls[0][2];
  useChatStore
    .getState()
    .setPendingInputs([{ requestId: 'wait', signalId: 'signal' }]);
  vi.mocked(sendSessionMessage).mockResolvedValue({
    ...original,
    state: 'queued',
    enqueuedAtMs: 1,
  });
  await act(async () =>
    expect(await result.current.sendMessage('Hello')).toBe(true)
  );
  expect(vi.mocked(sendSessionMessage).mock.calls[1][2]).toEqual(original);
  expect(submitInput).not.toHaveBeenCalled();
  useChatStore.getState().setPendingInputs([]);
  await act(() => result.current.sendMessage('Hello'));
  expect(vi.mocked(sendSessionMessage).mock.calls[2][2].operationId).not.toBe(
    original.operationId
  );
});

it('does not duplicate an in-flight enqueue or put an old-session error into a new session', async () => {
  let reject!: (error: Error) => void;
  vi.mocked(sendSessionMessage).mockReturnValue(
    new Promise((_resolve, fail) => {
      reject = fail;
    })
  );
  const { result } = renderHook(() => useChatStream('workflow'));
  const first = result.current.sendMessage('Hello');
  expect(await result.current.sendMessage('Hello')).toBe(false);
  expect(sendSessionMessage).toHaveBeenCalledTimes(1);
  useChatStore.getState().setSessionId('replacement');
  await act(async () => {
    reject(new Error('Old session failed'));
    await first;
  });
  expect(useChatStore.getState().error).toBeNull();
});
