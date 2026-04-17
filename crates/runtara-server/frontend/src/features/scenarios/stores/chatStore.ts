import { create } from 'zustand';
import { devtools } from 'zustand/middleware';
import { immer } from 'zustand/middleware/immer';
import {
  ChatMessage,
  ChatSSEEvent,
  ChatStatus,
  WaitingForInputData,
} from '@/features/scenarios/types/chat';

interface ChatStoreState {
  // Core state
  scenarioId: string | null;
  scenarioName: string | null;
  instanceId: string | null;
  sessionId: string | null;
  messages: ChatMessage[];
  status: ChatStatus;
  waitingForInput: WaitingForInputData | null;
  error: string | null;

  // Stream management (not serialized by immer — stored as plain ref)
  abortController: AbortController | null;

  // Actions
  initChat: (scenarioId: string, scenarioName: string) => void;
  resumeChat: (
    scenarioId: string,
    scenarioName: string,
    instanceId: string
  ) => void;
  addUserMessage: (content: string) => void;
  addSystemMessage: (content: string) => void;
  startAssistantMessage: () => void;
  appendToAssistantMessage: (content: string) => void;
  addEventToAssistantMessage: (event: ChatSSEEvent) => void;
  finalizeAssistantMessage: () => void;
  setStatus: (status: ChatStatus) => void;
  setInstanceId: (instanceId: string) => void;
  setSessionId: (token: string | null) => void;
  setWaitingForInput: (data: WaitingForInputData | null) => void;
  setError: (error: string | null) => void;
  setAbortController: (controller: AbortController | null) => void;
  loadHistory: (messages: ChatMessage[]) => void;
  resetChat: () => void;
}

let idCounter = 0;
function generateId(): string {
  idCounter += 1;
  return `msg_${Date.now()}_${idCounter}`;
}

export const useChatStore = create<ChatStoreState>()(
  devtools(
    immer((set) => ({
      // Initial state
      scenarioId: null,
      scenarioName: null,
      instanceId: null,
      sessionId: null,
      messages: [],
      status: 'idle' as ChatStatus,
      waitingForInput: null,
      error: null,
      abortController: null,

      // Actions
      initChat: (scenarioId, scenarioName) => {
        set((state) => {
          state.scenarioId = scenarioId;
          state.scenarioName = scenarioName;
          state.instanceId = null;
          state.sessionId = null;
          state.messages = [];
          state.status = 'idle';
          state.waitingForInput = null;
          state.error = null;
          state.abortController = null;
        });
      },

      resumeChat: (scenarioId, scenarioName, instanceId) => {
        set((state) => {
          state.scenarioId = scenarioId;
          state.scenarioName = scenarioName;
          state.instanceId = instanceId;
          state.sessionId = null;
          state.messages = [];
          state.status = 'idle';
          state.waitingForInput = null;
          state.error = null;
          state.abortController = null;
        });
      },

      addUserMessage: (content) => {
        set((state) => {
          state.messages.push({
            id: generateId(),
            role: 'user',
            content,
            timestamp: new Date().toISOString(),
            events: [],
          });
        });
      },

      addSystemMessage: (content) => {
        set((state) => {
          state.messages.push({
            id: generateId(),
            role: 'system',
            content,
            timestamp: new Date().toISOString(),
            events: [],
          });
        });
      },

      startAssistantMessage: () => {
        set((state) => {
          state.messages.push({
            id: generateId(),
            role: 'assistant',
            content: '',
            timestamp: new Date().toISOString(),
            events: [],
            isStreaming: true,
          });
        });
      },

      appendToAssistantMessage: (content) => {
        set((state) => {
          // Search backwards — system messages (memory_loaded etc.) may appear after the assistant placeholder
          for (let i = state.messages.length - 1; i >= 0; i--) {
            if (state.messages[i].role === 'assistant') {
              state.messages[i].content += content;
              break;
            }
          }
        });
      },

      addEventToAssistantMessage: (event) => {
        set((state) => {
          for (let i = state.messages.length - 1; i >= 0; i--) {
            if (state.messages[i].role === 'assistant') {
              state.messages[i].events.push(event);
              break;
            }
          }
        });
      },

      finalizeAssistantMessage: () => {
        set((state) => {
          for (let i = state.messages.length - 1; i >= 0; i--) {
            if (state.messages[i].role === 'assistant') {
              state.messages[i].isStreaming = false;
              break;
            }
          }
        });
      },

      setStatus: (status) => {
        set((state) => {
          state.status = status;
        });
      },

      setInstanceId: (instanceId) => {
        set((state) => {
          state.instanceId = instanceId;
        });
      },

      setSessionId: (token) => {
        set((state) => {
          state.sessionId = token;
        });
      },

      setWaitingForInput: (data) => {
        set((state) => {
          state.waitingForInput = data;
        });
      },

      setError: (error) => {
        set((state) => {
          state.error = error;
        });
      },

      setAbortController: (controller) => {
        set((state) => {
          state.abortController = controller;
        });
      },

      loadHistory: (messages) => {
        set((state) => {
          state.messages = messages;
        });
      },

      resetChat: () => {
        set((state) => {
          // Abort any in-flight stream
          state.abortController?.abort();
          state.scenarioId = null;
          state.scenarioName = null;
          state.instanceId = null;
          state.sessionId = null;
          state.messages = [];
          state.status = 'idle';
          state.waitingForInput = null;
          state.error = null;
          state.abortController = null;
        });
      },
    })),
    { name: 'chat-store' }
  )
);
