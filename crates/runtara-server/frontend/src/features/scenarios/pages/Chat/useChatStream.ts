import { useCallback, useRef } from 'react';
import { useToken } from '@/shared/hooks/useToken';
import { useChatStore } from '@/features/scenarios/stores/chatStore';
import {
  createChatSession,
  sendSessionMessage,
  reconnectSession,
  checkPendingInput,
} from '@/features/scenarios/queries/chat';
import { parseSSEStream } from '@/features/scenarios/utils/sse';
import { ChatSSEEvent } from '@/features/scenarios/types/chat';

/**
 * Processes a single SSE event and dispatches it to the chat store.
 */
function handleSSEEvent(event: ChatSSEEvent) {
  const store = useChatStore.getState();

  switch (event.type) {
    case 'session_created': {
      console.debug('[Chat] session_created event data:', event.data);
      const sessionId = (event.data.session_id ?? event.data.sessionId) as
        | string
        | undefined;
      const instanceId = (event.data.instance_id ?? event.data.instanceId) as
        | string
        | undefined;
      if (sessionId) {
        store.setSessionId(sessionId);
      }
      if (instanceId) {
        store.setInstanceId(instanceId);
      }
      break;
    }

    case 'started':
      if (typeof event.data.instance_id === 'string') {
        store.setInstanceId(event.data.instance_id);
      }
      store.addSystemMessage('Execution started');
      break;

    case 'memory_loaded': {
      const outputs = event.data.outputs as Record<string, unknown> | undefined;
      const count = event.data.message_count ?? outputs?.message_count ?? 0;
      store.addSystemMessage(`Loaded ${count} messages from memory`);
      break;
    }

    case 'llm_start': {
      const model = event.data.model as string | undefined;
      store.addSystemMessage(model ? `${model} thinking...` : 'Thinking...');
      break;
    }

    case 'llm_end':
      store.addSystemMessage('Done thinking');
      break;

    case 'tool_call': {
      const toolName = (event.data.tool_name as string) ?? 'tool';
      store.addSystemMessage(`Calling tool: ${toolName}`);
      break;
    }

    case 'tool_result': {
      const toolName2 = (event.data.tool_name as string) ?? 'tool';
      const durationMs = event.data.duration_ms as number | undefined;
      const durationStr =
        durationMs != null ? ` (${String(durationMs)}ms)` : '';
      store.addSystemMessage(`${toolName2} completed${durationStr}`);
      break;
    }

    case 'memory_saved': {
      const savedCount = event.data.message_count ?? 0;
      store.addSystemMessage(`Saved ${savedCount} messages to memory`);
      break;
    }

    case 'message':
      if (typeof event.data.content === 'string') {
        // Lazily create the assistant message so it appears after any
        // preceding system messages (tool_call, tool_result, etc.)
        const currentMsgs = useChatStore.getState().messages;
        const hasStreaming = currentMsgs.some(
          (m) => m.role === 'assistant' && m.isStreaming
        );
        if (!hasStreaming) {
          store.startAssistantMessage();
        }
        store.appendToAssistantMessage(event.data.content);
      }
      break;

    case 'waiting_for_input': {
      store.finalizeAssistantMessage();

      // Only show system message for structured input (not simple "message" field)
      const responseSchema = event.data.response_schema as
        | Record<string, unknown>
        | undefined;
      const schemaKeys =
        responseSchema && typeof responseSchema === 'object'
          ? Object.keys(responseSchema)
          : [];
      const isSimple =
        schemaKeys.length === 0 ||
        (schemaKeys.length === 1 && schemaKeys[0] === 'message');
      if (!isSimple) {
        const promptMsg =
          (event.data.message as string) ?? 'Waiting for input...';
        store.addSystemMessage(promptMsg);
      }

      store.setWaitingForInput({
        signalId: event.data.signal_id as string,
        message: event.data.message as string | undefined,
        responseSchema: event.data.response_schema as
          | Record<string, unknown>
          | undefined,
        toolName: event.data.tool_name as string | undefined,
      });
      store.setStatus('waiting_for_input');
      break;
    }

    case 'done':
      store.finalizeAssistantMessage();
      // In session mode, 'done' means one execution turn finished —
      // the session stays alive for the next user message.
      store.setStatus('idle');
      break;

    case 'error':
      store.setError(
        (event.data.message as string) ?? 'An unknown error occurred'
      );
      store.finalizeAssistantMessage();
      store.setStatus('error');
      break;
  }
}

export function useChatStream(scenarioId: string) {
  const token = useToken();
  const abortRef = useRef<AbortController | null>(null);

  /**
   * Shared helper: consumes an SSE Response stream and dispatches events.
   */
  const consumeStream = useCallback(
    async (response: Response, abortController: AbortController) => {
      const store = useChatStore.getState();

      if (!response.body) {
        throw new Error('Response body is null — SSE stream unavailable');
      }

      for await (const event of parseSSEStream(
        response.body,
        abortController.signal
      )) {
        handleSSEEvent(event);
      }

      // If stream ends without an explicit 'done' or 'error' event,
      // and we're still streaming, finalize gracefully
      const finalState = useChatStore.getState();
      if (finalState.status === 'streaming') {
        store.finalizeAssistantMessage();
        store.setStatus('idle');
      }
    },
    []
  );

  /**
   * Start a new chat session. Creates the instance on the backend
   * and opens the SSE stream. The AI agent initiates the conversation.
   */
  const startSession = useCallback(async () => {
    const store = useChatStore.getState();

    // Abort any previous stream
    abortRef.current?.abort();

    store.setStatus('streaming');
    store.setError(null);

    const abortController = new AbortController();
    abortRef.current = abortController;
    store.setAbortController(abortController);

    try {
      const response = await createChatSession(token, scenarioId, {
        signal: abortController.signal,
      });

      await consumeStream(response, abortController);
    } catch (err: unknown) {
      if (err instanceof DOMException && err.name === 'AbortError') {
        return;
      }
      const message =
        err instanceof Error ? err.message : 'Failed to start session';
      store.setError(message);
      store.setStatus('error');
    }
  }, [token, scenarioId, consumeStream]);

  /**
   * Reconnect to an existing session's SSE stream (e.g. after page refresh).
   */
  const reconnect = useCallback(
    async (sessionId: string) => {
      const store = useChatStore.getState();

      abortRef.current?.abort();

      store.setStatus('streaming');
      store.setError(null);

      const abortController = new AbortController();
      abortRef.current = abortController;
      store.setAbortController(abortController);

      try {
        const response = await reconnectSession(
          token,
          sessionId,
          abortController.signal
        );

        await consumeStream(response, abortController);
      } catch (err: unknown) {
        if (err instanceof DOMException && err.name === 'AbortError') {
          return;
        }
        const message =
          err instanceof Error ? err.message : 'Failed to reconnect';
        store.setError(message);
        store.setStatus('error');
      }
    },
    [token, consumeStream]
  );

  /**
   * Send a user message to the session.
   * Just POSTs the message — the SSE stream from startSession stays open
   * and will deliver the response events automatically.
   */
  const sendMessage = useCallback(
    async (content: string) => {
      const store = useChatStore.getState();
      const { sessionId, waitingForInput } = store;

      if (!sessionId) {
        store.setError('No active session');
        return;
      }

      // Add user message — assistant placeholder will be created lazily
      // when the first 'message' SSE event arrives, so it appears after
      // any tool_call/tool_result system messages.
      store.addUserMessage(content);
      store.setStatus('streaming');
      store.setError(null);

      // If we're responding to a waiting_for_input, clear it
      if (waitingForInput) {
        store.setWaitingForInput(null);
      }

      try {
        await sendSessionMessage(token, sessionId, content);
      } catch (err: unknown) {
        const message =
          err instanceof Error ? err.message : 'Failed to send message';
        store.setError(message);
        store.finalizeAssistantMessage();
        store.setStatus('error');
      }
    },
    [token]
  );

  /**
   * Restore the waiting_for_input state after a page refresh
   * by checking the pending-input endpoint.
   */
  const restorePendingInput = useCallback(
    async (sessionId: string) => {
      try {
        const pending = await checkPendingInput(token, sessionId);
        if (pending.hasPendingInput && pending.signalId) {
          const store = useChatStore.getState();
          store.setWaitingForInput({
            signalId: pending.signalId,
            message: pending.message,
            responseSchema: pending.responseSchema,
            toolName: pending.toolName,
          });
          store.setStatus('waiting_for_input');
        }
      } catch {
        // Non-critical — the SSE stream may deliver the event anyway
      }
    },
    [token]
  );

  const cancelStream = useCallback(() => {
    abortRef.current?.abort();
    abortRef.current = null;

    const store = useChatStore.getState();
    store.finalizeAssistantMessage();
    store.setAbortController(null);

    // Only change status if we were actively streaming
    if (store.status === 'streaming') {
      store.setStatus('idle');
    }
  }, []);

  return {
    startSession,
    reconnect,
    sendMessage,
    restorePendingInput,
    cancelStream,
  };
}
