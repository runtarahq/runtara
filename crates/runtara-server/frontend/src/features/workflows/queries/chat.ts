import { ChatMessage, ChatSSEEvent } from '@/features/workflows/types/chat';
import { getRuntimeBaseUrl } from '@/shared/queries/utils';

// ---------------------------------------------------------------------------
// Session-based chat API
// ---------------------------------------------------------------------------

export interface CreateSessionOptions {
  data?: Record<string, unknown>;
  variables?: Record<string, unknown>;
  version?: number | null;
  signal?: AbortSignal;
}

/**
 * Create a new chat session. Returns the raw SSE Response.
 * The stream will emit a `session_created` event with `token`, `sessionId`,
 * and `instanceId`, followed by regular chat events.
 */
export async function createChatSession(
  token: string,
  workflowId: string,
  options?: CreateSessionOptions
): Promise<Response> {
  const url = `${getRuntimeBaseUrl()}/workflows/${encodeURIComponent(workflowId)}/sessions`;

  const response = await fetch(url, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Accept: 'text/event-stream',
      Authorization: `Bearer ${token}`,
    },
    body: JSON.stringify({
      data: options?.data ?? {},
      variables: options?.variables ?? {},
      version: options?.version ?? null,
    }),
    signal: options?.signal,
  });

  if (!response.ok) {
    const errorText = await response.text().catch(() => response.statusText);
    throw new Error(`Session creation failed: ${errorText}`);
  }

  return response;
}

/**
 * Send a message to an existing session.
 * Uses the same JWT auth as all other runtime endpoints.
 */
export async function sendSessionMessage(
  token: string,
  sessionId: string,
  message: string,
  signal?: AbortSignal
): Promise<void> {
  const url = `${getRuntimeBaseUrl()}/sessions/${encodeURIComponent(sessionId)}/events`;

  const response = await fetch(url, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${token}`,
    },
    body: JSON.stringify({ message }),
    signal,
  });

  if (!response.ok) {
    const errorText = await response.text().catch(() => response.statusText);
    throw new Error(`Failed to send message: ${errorText}`);
  }
}

/**
 * Reconnect to an existing session's SSE stream (e.g. after page refresh).
 * Returns the raw Response so the caller can consume the SSE stream.
 */
export async function reconnectSession(
  token: string,
  sessionId: string,
  signal?: AbortSignal
): Promise<Response> {
  const url = `${getRuntimeBaseUrl()}/sessions/${encodeURIComponent(sessionId)}/events`;

  const response = await fetch(url, {
    method: 'GET',
    headers: {
      Accept: 'text/event-stream',
      Authorization: `Bearer ${token}`,
    },
    signal,
  });

  if (!response.ok) {
    const errorText = await response.text().catch(() => response.statusText);
    throw new Error(`Session reconnect failed: ${errorText}`);
  }

  return response;
}

export interface PendingInputResponse {
  hasPendingInput: boolean;
  signalId?: string;
  message?: string;
  responseSchema?: Record<string, unknown>;
  toolName?: string;
}

/**
 * Check if a session has a pending input request (used after page refresh
 * to restore the waiting_for_input state).
 */
export async function checkPendingInput(
  token: string,
  sessionId: string
): Promise<PendingInputResponse> {
  const url = `${getRuntimeBaseUrl()}/sessions/${encodeURIComponent(sessionId)}/pending-input`;

  const response = await fetch(url, {
    method: 'GET',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${token}`,
    },
  });

  if (!response.ok) {
    throw new Error(`Failed to check pending input: ${response.statusText}`);
  }

  return response.json();
}

/**
 * Reconstruct chat history from instance step events.
 * Parses SSE-like events from the step events API and builds ChatMessage[].
 */
export async function fetchChatHistory(
  token: string,
  workflowId: string,
  instanceId: string
): Promise<ChatMessage[]> {
  const url = `${getRuntimeBaseUrl()}/workflows/${encodeURIComponent(workflowId)}/instances/${encodeURIComponent(instanceId)}/step-events?sortOrder=asc&limit=1000`;

  const response = await fetch(url, {
    method: 'GET',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${token}`,
    },
  });

  if (!response.ok) {
    throw new Error(`Failed to fetch chat history: ${response.statusText}`);
  }

  const json = await response.json();
  const events: Record<string, unknown>[] =
    json?.data?.events ?? json?.events ?? [];

  return reconstructMessagesFromEvents(events);
}

/**
 * Convert raw step events from the step-events API into ChatMessage objects.
 * Each significant event becomes its own message in the chat timeline.
 *
 * Step events have this shape:
 *   { eventType, subtype?, payload?, createdAt, id }
 */
function reconstructMessagesFromEvents(
  events: Record<string, unknown>[]
): ChatMessage[] {
  const messages: ChatMessage[] = [];
  let msgCounter = 0;

  const makeId = () => {
    msgCounter += 1;
    return `history_${msgCounter}`;
  };

  const addSystem = (
    content: string,
    timestamp: string,
    sseEvents: ChatSSEEvent[] = []
  ) => {
    messages.push({
      id: makeId(),
      role: 'system',
      content,
      timestamp,
      events: sseEvents,
    });
  };

  for (const event of events) {
    const eventType = event.eventType as string | undefined;
    const subtype = event.subtype as string | undefined;
    const payload = event.payload as Record<string, unknown> | undefined;
    const createdAt = (event.createdAt as string) ?? new Date().toISOString();

    if (!eventType) continue;

    // Skip heartbeats
    if (eventType === 'heartbeat') continue;

    // Execution started
    if (eventType === 'started') {
      addSystem('Execution started', createdAt);
      continue;
    }

    // Execution completed
    if (eventType === 'completed') {
      addSystem('Completed', createdAt);
      continue;
    }

    // Execution failed
    if (eventType === 'failed') {
      addSystem(
        `Error: ${(payload?.message as string) ?? 'Execution failed'}`,
        createdAt
      );
      continue;
    }

    // Custom events — examine subtype
    if (eventType === 'custom' && subtype && payload) {
      const stepType = payload.step_type as string | undefined;
      const stepName = payload.step_name as string | undefined;
      const durationMs = payload.duration_ms as number | undefined;

      // External input requested (WaitForSignal)
      if (subtype === 'external_input_requested') {
        const waitEvent: ChatSSEEvent = {
          type: 'waiting_for_input',
          data: {
            signal_id: payload.signal_id,
            message: payload.message,
            response_schema: payload.response_schema,
            tool_name: payload.tool_name,
          },
          timestamp: createdAt,
        };
        const promptMsg = (payload.message as string) ?? 'Waiting for input...';
        addSystem(promptMsg, createdAt, [waitEvent]);
        continue;
      }

      // Step debug start
      if (subtype === 'step_debug_start') {
        if (stepType === 'AiAgentMemoryLoad') {
          addSystem('Loading memory...', createdAt);
        } else if (stepType === 'AiAgentToolCall') {
          const inputs = payload.inputs as Record<string, unknown> | undefined;
          const toolName = (inputs?.tool_name as string) ?? stepName ?? 'tool';
          addSystem(`Calling tool: ${toolName}`, createdAt);
        } else if (stepType === 'AiAgentLlmCall' || stepType === 'AiAgent') {
          addSystem(`${stepName ?? 'AI Agent'} thinking...`, createdAt);
        } else if (stepType === 'AiAgentMemorySave') {
          addSystem('Saving memory...', createdAt);
        } else {
          addSystem(`${stepName ?? stepType ?? 'Step'} started`, createdAt);
        }
        continue;
      }

      // Step debug end
      if (subtype === 'step_debug_end') {
        const durationStr =
          durationMs != null ? ` (${String(durationMs)}ms)` : '';

        if (stepType === 'AiAgentMemoryLoad') {
          const outputs = payload.outputs as
            | Record<string, unknown>
            | undefined;
          const memCount = outputs?.message_count ?? 0;
          addSystem(
            `Loaded ${memCount} messages from memory${durationStr}`,
            createdAt
          );
        } else if (stepType === 'AiAgentToolCall') {
          const toolLabel = (stepName ?? 'Tool').replace('Tool: ', '');
          addSystem(`${toolLabel} completed${durationStr}`, createdAt);
        } else if (stepType === 'AiAgentMemorySave') {
          addSystem(`Memory saved${durationStr}`, createdAt);
        } else if (stepType === 'AiAgentLlmCall' || stepType === 'AiAgent') {
          addSystem(`${stepName ?? 'AI Agent'} done${durationStr}`, createdAt);
        } else {
          addSystem(
            `${stepName ?? stepType ?? 'Step'} completed${durationStr}`,
            createdAt
          );
        }
        continue;
      }

      // External input received (user responded to signal)
      if (subtype === 'external_input_received') {
        const responseData = payload.response ?? payload.data;
        messages.push({
          id: makeId(),
          role: 'user',
          content:
            typeof responseData === 'string'
              ? responseData
              : JSON.stringify(responseData ?? 'Response sent'),
          timestamp: createdAt,
          events: [],
        });
        continue;
      }
    }

    // Message events (if the backend emits them as step events)
    if (eventType === 'message') {
      const content =
        payload?.content ?? (event as Record<string, unknown>).content;
      messages.push({
        id: makeId(),
        role: 'assistant',
        content: (content as string) ?? '',
        timestamp: createdAt,
        events: [],
      });
      continue;
    }
  }

  return messages;
}
