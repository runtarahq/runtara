import { useEffect, useCallback, useRef } from 'react';
import { useParams, useNavigate } from 'react-router';
import { ArrowLeft } from 'lucide-react';
import { Button } from '@/shared/components/ui/button.tsx';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { getScenario, getScenarioInstance } from '@/features/scenarios/queries';
import { fetchChatHistory } from '@/features/scenarios/queries/chat';
import { useToken } from '@/shared/hooks/useToken';
import { useChatStore } from '@/features/scenarios/stores/chatStore';
import { useChatStream } from './useChatStream';
import { ChatMessageList } from '@/features/scenarios/components/ChatMessageList';
import { ChatInput } from '@/features/scenarios/components/ChatInput';
import { toast } from 'sonner';

export function ChatPage() {
  const { scenarioId, instanceId } = useParams<{
    scenarioId: string;
    instanceId?: string;
  }>();
  const navigate = useNavigate();
  const token = useToken();

  // Fetch scenario details for the header
  const { data: scenarioResponse } = useCustomQuery({
    queryKey: queryKeys.scenarios.byId(scenarioId ?? ''),
    queryFn: (token: string) => getScenario(token, scenarioId!),
    enabled: !!scenarioId,
  });

  const scenarioName = (scenarioResponse as any)?.data?.name ?? 'Chat';

  usePageTitle(`Chat - ${scenarioName}`);

  // Chat store state
  const messages = useChatStore((s) => s.messages);
  const status = useChatStore((s) => s.status);
  const waitingForInput = useChatStore((s) => s.waitingForInput);
  const error = useChatStore((s) => s.error);
  const storeInstanceId = useChatStore((s) => s.instanceId);

  // Chat stream actions
  const {
    startSession,
    reconnect,
    sendMessage,
    restorePendingInput,
    cancelStream,
  } = useChatStream(scenarioId ?? '');

  // Guard against StrictMode double-mount and dependency-triggered re-runs
  const initRef = useRef(false);

  // Initialize or resume chat on mount
  useEffect(() => {
    if (!scenarioId) return;
    if (initRef.current) return;
    initRef.current = true;

    const store = useChatStore.getState();

    if (instanceId) {
      store.resumeChat(scenarioId, scenarioName, instanceId);

      // Fetch instance detail to get sessionId from inputs.data.sessionId,
      // and load chat history in parallel
      Promise.all([
        getScenarioInstance(token, scenarioId, instanceId),
        fetchChatHistory(token, scenarioId, instanceId),
      ])
        .then(([instanceData, historyMessages]) => {
          store.loadHistory(historyMessages);

          // Check if the last event was a waiting_for_input
          const lastSystemMsg = historyMessages
            .filter((m) => m.role === 'system')
            .pop();
          const waitEvent = lastSystemMsg?.events.find(
            (e) => e.type === 'waiting_for_input'
          );
          if (waitEvent) {
            store.setWaitingForInput({
              signalId: waitEvent.data.signal_id as string,
              message: waitEvent.data.message as string | undefined,
              responseSchema: waitEvent.data.response_schema as
                | Record<string, unknown>
                | undefined,
              toolName: waitEvent.data.tool_name as string | undefined,
            });
            store.setStatus('waiting_for_input');
          }

          // Extract sessionId from instance inputs and reconnect
          const sessionId = instanceData?.inputs?.data?.sessionId as
            | string
            | undefined;
          if (sessionId) {
            store.setSessionId(sessionId);
            reconnect(sessionId);
            restorePendingInput(sessionId);
          }
        })
        .catch(() => {
          toast.error('Failed to load chat history');
        });
    } else {
      store.initChat(scenarioId, scenarioName);
      // Start a new session — AI agent initiates the conversation
      startSession();
    }

    return () => {
      initRef.current = false;
      cancelStream();
      useChatStore.getState().resetChat();
    };
  }, [scenarioId, instanceId]); // eslint-disable-line react-hooks/exhaustive-deps

  const handleBack = useCallback(() => {
    navigate(`/scenarios/${scenarioId}`);
  }, [navigate, scenarioId]);

  return (
    <div className="flex h-dvh flex-col overflow-hidden">
      {/* Header */}
      <div className="flex items-center gap-3 border-b px-4 py-3">
        <Button
          variant="ghost"
          size="icon"
          onClick={handleBack}
          className="h-8 w-8"
        >
          <ArrowLeft className="h-4 w-4" />
        </Button>
        <div className="flex-1 min-w-0">
          <h1 className="truncate text-sm font-semibold">{scenarioName}</h1>
          {useChatStore.getState().instanceId && (
            <p className="truncate text-xs text-muted-foreground">
              Instance: {useChatStore.getState().instanceId}
            </p>
          )}
        </div>
        {status === 'done' && (
          <span className="text-xs text-muted-foreground">Completed</span>
        )}
      </div>

      {/* Error banner */}
      {error && (
        <div className="mx-4 mt-2 rounded-lg border border-destructive/50 bg-destructive/10 px-3 py-2 text-xs text-destructive">
          {error}
        </div>
      )}

      {/* Message list */}
      <ChatMessageList messages={messages} />

      {/* Input */}
      <ChatInput
        onSend={sendMessage}
        onSignalResponse={sendMessage}
        status={status}
        waitingForInput={waitingForInput}
        instanceId={storeInstanceId}
        token={token}
      />
    </div>
  );
}
