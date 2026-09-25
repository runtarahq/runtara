import { useCallback, useEffect, useRef, useState } from 'react';
import { useChatStore } from '../../stores/chatStore';
import { checkPendingInput } from '../../queries/chat';
import { deliverSignal, getPendingInput } from '../../queries';
import {
  InputSubmissionError,
  InputSubmissionTracker,
  isStaleInputSubmission,
} from '../../utils/input-submission';
import type { WaitingForInputData } from '../../types/chat';

interface UncertainResponse {
  operationId: string;
  instanceId: string;
  sessionId: string | null;
  workflowId: string;
  request: WaitingForInputData;
  payload: Record<string, unknown>;
}

/** Discovery is independent of optional SSE/debug events and the stream lifetime. */
export function useChatInputs(workflowId: string, token: string) {
  const instanceId = useChatStore((state) => state.instanceId);
  const sessionId = useChatStore((state) => state.sessionId);
  const generation = useRef(0);
  const submitting = useRef(false);
  const submissions = useRef(new InputSubmissionTracker());
  const uncertain = useRef(new Map<string, UncertainResponse>());
  const [uncertainResponses, setUncertainResponses] = useState<
    UncertainResponse[]
  >([]);

  const refresh = useCallback(
    async (signal?: AbortSignal): Promise<boolean> => {
      const owner = useChatStore.getState();
      if (
        !owner.instanceId ||
        owner.workflowId !== workflowId ||
        submitting.current
      )
        return false;
      const version = ++generation.current;
      const isCurrent = () => {
        const current = useChatStore.getState();
        return (
          !signal?.aborted &&
          version === generation.current &&
          current.workflowId === workflowId &&
          current.instanceId === owner.instanceId &&
          current.sessionId === owner.sessionId
        );
      };
      try {
        const page = owner.sessionId
          ? await checkPendingInput(token, owner.sessionId, signal)
          : {
              instanceId: owner.instanceId,
              pendingInputs: await getPendingInput(
                token,
                workflowId,
                owner.instanceId,
                signal
              ),
            };
        if (!isCurrent()) return false;
        // Session routing can advance while a prior execution is still displayed.
        if (page.instanceId !== owner.instanceId) {
          owner.setInstanceId(page.instanceId);
        }
        const pending: WaitingForInputData[] = page.pendingInputs.map(
          (request) => ({ ...request, instanceId: page.instanceId })
        );
        const retries = uncertain.current.values();
        // A lost acceptance acknowledgement must remain retryable even when discovery
        // already excludes the accepted request. Keep its original form until resolved.
        for (const retry of retries) {
          if (
            retry.workflowId === workflowId &&
            retry.sessionId === owner.sessionId &&
            (owner.sessionId !== null ||
              retry.instanceId === owner.instanceId) &&
            !pending.some(
              (item) =>
                item.requestId === retry.request.requestId &&
                item.instanceId === retry.instanceId
            )
          )
            pending.push(retry.request);
        }
        owner.setPendingInputs(pending);
        return true;
      } catch (error) {
        if (isCurrent())
          owner.setPendingInputError(
            error instanceof Error
              ? error.message
              : 'Pending inputs are unavailable'
          );
        return false;
      }
    },
    [workflowId, token]
  );

  useEffect(() => {
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let delay = 3000;
    const poll = async () => {
      const succeeded = await refresh(controller.signal);
      delay = succeeded ? 3000 : Math.min(delay * 2, 30000);
      if (!controller.signal.aborted) timer = setTimeout(poll, delay);
    };
    if (instanceId) void poll();
    return () => {
      controller.abort();
      generation.current += 1;
      clearTimeout(timer);
    };
  }, [instanceId, sessionId, refresh]);

  const submitInput = useCallback(
    async (
      requestId: string,
      payload: Record<string, unknown>,
      instanceId?: string,
      retryOperationId?: string
    ): Promise<boolean> => {
      const owner = useChatStore.getState();
      const retained = retryOperationId
        ? uncertain.current.get(retryOperationId)
        : undefined;
      const request =
        owner.pendingInputs.find(
          (item) =>
            item.requestId === requestId &&
            (!instanceId || item.instanceId === instanceId)
        ) ??
        (retained?.sessionId === owner.sessionId &&
        retained?.workflowId === workflowId
          ? retained.request
          : undefined);
      if (!owner.instanceId || !request || submitting.current) return false;
      const targetInstanceId = request.instanceId ?? owner.instanceId;
      const submission =
        retained &&
        retained.instanceId === targetInstanceId &&
        retained.request.requestId === requestId
          ? {
              requestId,
              operationId: retained.operationId,
              payload: retained.payload,
            }
          : submissions.current.prepare(targetInstanceId, requestId, payload);
      const retryKey = submission.operationId;
      const isOtherRequest = (item: WaitingForInputData) =>
        item.requestId !== requestId ||
        (item.instanceId ?? owner.instanceId) !== targetInstanceId;
      generation.current += 1;
      const isCurrent = () => {
        const current = useChatStore.getState();
        return (
          current.workflowId === owner.workflowId &&
          current.sessionId === owner.sessionId &&
          (owner.sessionId !== null || current.instanceId === owner.instanceId)
        );
      };
      submitting.current = true;
      owner.setError(null);
      try {
        await deliverSignal(token, targetInstanceId, submission);
        if (!isCurrent()) return false;
        uncertain.current.delete(retryKey);
        owner.setPendingInputs(
          useChatStore.getState().pendingInputs.filter(isOtherRequest)
        );
        return true;
      } catch (error) {
        if (!isCurrent()) return false;
        if (isStaleInputSubmission(error)) {
          uncertain.current.delete(retryKey);
          owner.setPendingInputs(
            useChatStore.getState().pendingInputs.filter(isOtherRequest)
          );
        } else if (
          !(error instanceof InputSubmissionError) ||
          !error.status ||
          error.status >= 500
        ) {
          uncertain.current.set(retryKey, {
            operationId: submission.operationId,
            instanceId: targetInstanceId,
            sessionId: owner.sessionId,
            workflowId,
            request: { ...request, instanceId: targetInstanceId },
            payload: submission.payload,
          });
        }
        owner.setError(
          error instanceof Error
            ? error.message
            : 'Response could not be confirmed. Retry the same response.'
        );
        return false;
      } finally {
        submitting.current = false;
        setUncertainResponses([...uncertain.current.values()]);
        if (isCurrent()) void refresh();
      }
    },
    [refresh, token, workflowId]
  );

  return {
    refreshPendingInput: refresh,
    submitInput,
    uncertainResponses: uncertainResponses.filter(
      (response) =>
        response.workflowId === workflowId &&
        response.sessionId === sessionId &&
        (sessionId !== null || response.instanceId === instanceId)
    ),
  };
}
