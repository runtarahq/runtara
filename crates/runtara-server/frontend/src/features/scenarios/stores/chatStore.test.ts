import { describe, it, expect, beforeEach } from 'vitest';
import { useChatStore } from './chatStore';
import { ChatMessage, ChatSSEEvent } from '@/features/scenarios/types/chat';

describe('chatStore', () => {
  beforeEach(() => {
    useChatStore.getState().resetChat();
  });

  describe('initChat', () => {
    it('sets scenarioId and scenarioName, resets messages', () => {
      useChatStore.getState().addUserMessage('old message');
      useChatStore.getState().initChat('scen-1', 'My Scenario');

      const state = useChatStore.getState();
      expect(state.scenarioId).toBe('scen-1');
      expect(state.scenarioName).toBe('My Scenario');
      expect(state.messages).toHaveLength(0);
      expect(state.status).toBe('idle');
      expect(state.instanceId).toBeNull();
    });
  });

  describe('resumeChat', () => {
    it('sets scenarioId, scenarioName, and instanceId', () => {
      useChatStore.getState().resumeChat('scen-1', 'My Scenario', 'inst-1');

      const state = useChatStore.getState();
      expect(state.scenarioId).toBe('scen-1');
      expect(state.instanceId).toBe('inst-1');
      expect(state.messages).toHaveLength(0);
    });
  });

  describe('addUserMessage', () => {
    it('adds a message with role user', () => {
      useChatStore.getState().addUserMessage('Hello');

      const state = useChatStore.getState();
      expect(state.messages).toHaveLength(1);
      expect(state.messages[0].role).toBe('user');
      expect(state.messages[0].content).toBe('Hello');
      expect(state.messages[0].id).toBeTruthy();
      expect(state.messages[0].events).toEqual([]);
    });
  });

  describe('addSystemMessage', () => {
    it('adds a message with role system', () => {
      useChatStore.getState().addSystemMessage('Loaded 5 messages');

      const state = useChatStore.getState();
      expect(state.messages).toHaveLength(1);
      expect(state.messages[0].role).toBe('system');
      expect(state.messages[0].content).toBe('Loaded 5 messages');
    });
  });

  describe('assistant message lifecycle', () => {
    it('starts, appends, and finalizes an assistant message', () => {
      const store = useChatStore.getState();
      store.startAssistantMessage();

      let state = useChatStore.getState();
      expect(state.messages).toHaveLength(1);
      expect(state.messages[0].role).toBe('assistant');
      expect(state.messages[0].content).toBe('');
      expect(state.messages[0].isStreaming).toBe(true);

      store.appendToAssistantMessage('Hello');
      state = useChatStore.getState();
      expect(state.messages[0].content).toBe('Hello');

      store.appendToAssistantMessage(' World');
      state = useChatStore.getState();
      expect(state.messages[0].content).toBe('Hello World');

      store.finalizeAssistantMessage();
      state = useChatStore.getState();
      expect(state.messages[0].isStreaming).toBe(false);
    });
  });

  describe('assistant message with interleaved system messages', () => {
    it('appends content to assistant even when system message is last', () => {
      const store = useChatStore.getState();
      store.startAssistantMessage();
      store.addSystemMessage('Loaded 0 messages from memory');
      store.appendToAssistantMessage('Hello from AI');

      const state = useChatStore.getState();
      expect(state.messages).toHaveLength(2);
      expect(state.messages[0].role).toBe('assistant');
      expect(state.messages[0].content).toBe('Hello from AI');
      expect(state.messages[1].role).toBe('system');
    });

    it('adds events to assistant even when system message is last', () => {
      const store = useChatStore.getState();
      store.startAssistantMessage();
      store.addSystemMessage('Loaded memory');

      const event: ChatSSEEvent = {
        type: 'tool_call',
        data: { tool_name: 'search' },
        timestamp: new Date().toISOString(),
      };
      store.addEventToAssistantMessage(event);

      const state = useChatStore.getState();
      expect(state.messages[0].events).toHaveLength(1);
      expect(state.messages[0].events[0].data.tool_name).toBe('search');
    });

    it('finalizes assistant even when system message is last', () => {
      const store = useChatStore.getState();
      store.startAssistantMessage();
      store.addSystemMessage('Memory saved');
      store.finalizeAssistantMessage();

      const state = useChatStore.getState();
      expect(state.messages[0].isStreaming).toBe(false);
    });
  });

  describe('addEventToAssistantMessage', () => {
    it('adds events to the last assistant message', () => {
      const store = useChatStore.getState();
      store.startAssistantMessage();

      const event: ChatSSEEvent = {
        type: 'tool_call',
        data: { tool_name: 'get_products' },
        timestamp: new Date().toISOString(),
      };

      store.addEventToAssistantMessage(event);

      const state = useChatStore.getState();
      expect(state.messages[0].events).toHaveLength(1);
      expect(state.messages[0].events[0].type).toBe('tool_call');
    });

    it('does not add events when last message is not assistant', () => {
      const store = useChatStore.getState();
      store.addUserMessage('hello');

      const event: ChatSSEEvent = {
        type: 'tool_call',
        data: { tool_name: 'test' },
        timestamp: new Date().toISOString(),
      };

      store.addEventToAssistantMessage(event);

      const state = useChatStore.getState();
      // The user message should not have events added
      expect(state.messages[0].events).toHaveLength(0);
    });
  });

  describe('setWaitingForInput', () => {
    it('sets and clears waiting for input data', () => {
      const store = useChatStore.getState();

      store.setWaitingForInput({
        signalId: 'sig-1',
        message: 'What product?',
        toolName: 'ask_user',
      });

      let state = useChatStore.getState();
      expect(state.waitingForInput).toEqual({
        signalId: 'sig-1',
        message: 'What product?',
        toolName: 'ask_user',
      });

      store.setWaitingForInput(null);
      state = useChatStore.getState();
      expect(state.waitingForInput).toBeNull();
    });
  });

  describe('loadHistory', () => {
    it('replaces all messages', () => {
      const store = useChatStore.getState();
      store.addUserMessage('old');

      const history: ChatMessage[] = [
        {
          id: 'h1',
          role: 'user',
          content: 'Hello',
          timestamp: new Date().toISOString(),
          events: [],
        },
        {
          id: 'h2',
          role: 'assistant',
          content: 'Hi there!',
          timestamp: new Date().toISOString(),
          events: [],
        },
      ];

      store.loadHistory(history);

      const state = useChatStore.getState();
      expect(state.messages).toHaveLength(2);
      expect(state.messages[0].id).toBe('h1');
      expect(state.messages[1].id).toBe('h2');
    });
  });

  describe('setStatus', () => {
    it('updates status', () => {
      useChatStore.getState().setStatus('streaming');
      expect(useChatStore.getState().status).toBe('streaming');

      useChatStore.getState().setStatus('done');
      expect(useChatStore.getState().status).toBe('done');
    });
  });

  describe('setInstanceId', () => {
    it('updates instanceId', () => {
      useChatStore.getState().setInstanceId('inst-123');
      expect(useChatStore.getState().instanceId).toBe('inst-123');
    });
  });

  describe('setError', () => {
    it('sets and clears error', () => {
      useChatStore.getState().setError('Something broke');
      expect(useChatStore.getState().error).toBe('Something broke');

      useChatStore.getState().setError(null);
      expect(useChatStore.getState().error).toBeNull();
    });
  });

  describe('resetChat', () => {
    it('resets all state to initial values', () => {
      const store = useChatStore.getState();
      store.initChat('scen-1', 'Test');
      store.addUserMessage('hello');
      store.setInstanceId('inst-1');
      store.setStatus('streaming');
      store.setError('oops');
      store.setWaitingForInput({ signalId: 'sig-1' });

      store.resetChat();

      const state = useChatStore.getState();
      expect(state.scenarioId).toBeNull();
      expect(state.scenarioName).toBeNull();
      expect(state.instanceId).toBeNull();
      expect(state.messages).toHaveLength(0);
      expect(state.status).toBe('idle');
      expect(state.waitingForInput).toBeNull();
      expect(state.error).toBeNull();
    });
  });

  describe('message ordering', () => {
    it('maintains correct order for mixed message types', () => {
      const store = useChatStore.getState();
      store.addUserMessage('Question');
      store.startAssistantMessage();
      store.appendToAssistantMessage('Answer');
      store.finalizeAssistantMessage();
      store.addSystemMessage('Done in 2s');

      const state = useChatStore.getState();
      expect(state.messages).toHaveLength(3);
      expect(state.messages[0].role).toBe('user');
      expect(state.messages[1].role).toBe('assistant');
      expect(state.messages[2].role).toBe('system');
    });
  });
});
