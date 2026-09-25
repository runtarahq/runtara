import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { SessionDeliveries } from './SessionDeliveries';
import { useChatStore } from '../../stores/chatStore';
import {
  listSessionDeliveries,
  resolveSessionDelivery,
} from '../../queries/chat';
import type { DeliveryStatus } from '@/generated/RuntaraRuntimeApi';

vi.mock('../../queries/chat', () => ({
  listSessionDeliveries: vi.fn(),
  resolveSessionDelivery: vi.fn(),
}));
const blocked: DeliveryStatus = {
  messageId: 'message-one',
  operationId: 'operation',
  state: 'blocked',
  reason: 'ambiguous_target',
  enqueuedAtMs: 1,
};
beforeEach(() => {
  vi.resetAllMocks();
  useChatStore.getState().resetChat();
  useChatStore.getState().resumeChat('workflow', 'Workflow', 'instance');
  useChatStore.getState().setSessionId('session');
  useChatStore.getState().setPendingInputs([
    {
      requestId: 'request',
      instanceId: 'instance',
      signalId: 'signal',
      message: 'Approval',
    },
  ]);
  vi.mocked(listSessionDeliveries).mockResolvedValue({
    deliveries: [blocked],
    nextCursor: '0',
  });
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

it('restores blocked deliveries without SSE and submits an explicit authorized target', async () => {
  render(<SessionDeliveries token="token" />);
  vi.mocked(resolveSessionDelivery).mockResolvedValue({
    ...blocked,
    state: 'queued',
  });
  fireEvent.click(
    await screen.findByRole('button', { name: 'Send to Approval' })
  );
  await waitFor(() =>
    expect(resolveSessionDelivery).toHaveBeenCalledWith(
      'token',
      'session',
      'message-one',
      { action: 'select', instanceId: 'instance', requestId: 'request' }
    )
  );
});

it('keeps a bound target immutable and exposes failures instead of hiding the delivery', async () => {
  vi.mocked(listSessionDeliveries).mockResolvedValue({
    deliveries: [
      { ...blocked, instanceId: 'original', requestId: 'original-request' },
    ],
    nextCursor: '0',
  });
  vi.mocked(resolveSessionDelivery).mockRejectedValue(
    new Error('Service unavailable')
  );
  render(<SessionDeliveries token="token" />);
  fireEvent.click(
    await screen.findByRole('button', { name: 'Retry original request' })
  );
  await waitFor(() =>
    expect(resolveSessionDelivery).toHaveBeenCalledWith(
      'token',
      'session',
      'message-one',
      {
        action: 'select',
        instanceId: 'original',
        requestId: 'original-request',
      }
    )
  );
  expect(
    screen.queryByRole('button', { name: 'Send to Approval' })
  ).not.toBeInTheDocument();
  vi.mocked(listSessionDeliveries).mockRejectedValue(
    new Error('Status unavailable')
  );
  await act(async () => {});
  expect(screen.getByText('Message message-: blocked')).toBeInTheDocument();
  expect(screen.getByRole('alert')).toHaveTextContent('Service unavailable');
});

it('scans subsequent pages, deduplicates messages, and ignores delayed old-session responses', async () => {
  vi.useFakeTimers();
  vi.mocked(listSessionDeliveries)
    .mockResolvedValueOnce({
      deliveries: [blocked],
      nextCursor: '18446744073709551615',
    })
    .mockResolvedValueOnce({
      deliveries: [
        blocked,
        { ...blocked, messageId: 'message-two', state: 'failed' },
      ],
      nextCursor: '0',
    });
  render(<SessionDeliveries token="token" />);
  await act(async () => {});
  await act(() => vi.advanceTimersByTimeAsync(1000));
  expect(vi.mocked(listSessionDeliveries).mock.calls[1][2]).toBe(
    '18446744073709551615'
  );
  expect(screen.getByText('Message delivery (2 loaded)')).toBeInTheDocument();
  let finish!: (page: {
    deliveries: DeliveryStatus[];
    nextCursor: string;
  }) => void;
  vi.mocked(listSessionDeliveries).mockReturnValueOnce(
    new Promise((resolve) => {
      finish = resolve;
    })
  );
  await act(() => vi.advanceTimersByTimeAsync(3000));
  vi.mocked(listSessionDeliveries).mockResolvedValue({
    deliveries: [],
    nextCursor: '0',
  });
  await act(async () => useChatStore.getState().setSessionId('replacement'));
  await act(async () => finish({ deliveries: [blocked], nextCursor: '0' }));
  expect(screen.queryByText(/Message delivery/)).not.toBeInTheDocument();
});
