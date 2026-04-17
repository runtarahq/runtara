// Chat feature type definitions

export type ChatSSEEventType =
  | 'session_created'
  | 'started'
  | 'memory_loaded'
  | 'llm_start'
  | 'llm_end'
  | 'tool_call'
  | 'tool_result'
  | 'waiting_for_input'
  | 'message'
  | 'memory_saved'
  | 'done'
  | 'error';

export interface ChatSSEEvent {
  type: ChatSSEEventType;
  data: Record<string, unknown>;
  timestamp: string;
}

export type ChatMessageRole = 'user' | 'assistant' | 'system';

export interface ChatMessage {
  id: string;
  role: ChatMessageRole;
  content: string;
  timestamp: string;
  /** SSE events associated with this message (tool calls, memory ops, etc.) */
  events: ChatSSEEvent[];
  /** Whether the message is still being streamed */
  isStreaming?: boolean;
}

export type ChatStatus =
  | 'idle'
  | 'streaming'
  | 'waiting_for_input'
  | 'done'
  | 'error';

export interface WaitingForInputData {
  signalId: string;
  message?: string;
  responseSchema?: Record<string, unknown>;
  toolName?: string;
}
