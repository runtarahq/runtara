import { useEffect, useCallback, useRef } from 'react';
import { useParams, useNavigate } from 'react-router';
import { ArrowLeft, MessageSquareOff } from 'lucide-react';
import { WorkflowDto } from '@/generated/RuntaraRuntimeApi';
import { Button } from '@/shared/components/ui/button.tsx';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { getWorkflow, getWorkflowInstance } from '@/features/workflows/queries';
import { fetchChatHistory } from '@/features/workflows/queries/chat';
import { useToken } from '@/shared/hooks/useToken';
import { useChatStore } from '@/features/workflows/stores/chatStore';
import { useChatStream } from './useChatStream';
import { ChatMessageList } from '@/features/workflows/components/ChatMessageList';
import { ChatInput } from '@/features/workflows/components/ChatInput';
import { toast } from 'sonner';

export function ChatPage() {
  const { workflowId, instanceId } = useParams<{
    workflowId: string;
    instanceId?: string;
  }>();
  const navigate = useNavigate();
  const token = useToken();

  // Fetch workflow details for the header
  const { data: workflowResponse } = useCustomQuery({
    queryKey: queryKeys.workflows.byId(workflowId ?? ''),
    queryFn: (token: string) => getWorkflow(token, workflowId!),
    enabled: !!workflowId,
  });

  const workflow = (workflowResponse as { data?: WorkflowDto } | undefined)
    ?.data;
  const workflowName = workflow?.name ?? 'Chat';

  // The server reports whether the graph contains a step that waits for a
  // reply. Without one nothing ever reads what is typed here, so starting a
  // session is a dead end and the page says so instead. Direct URLs still land
  // here — this is the fallback for them, not a redirect.
  //
  // Resuming a run (`:instanceId`) is exempt: it already has a transcript, and
  // Invocation History only links here for a run that is holding a pending
  // input. Refusing that would block the one reply the user came to give.
  const isWorkflowLoaded = !!workflow;
  const supportsChat = workflow?.supportsChat === true;
  const isDeadEnd = isWorkflowLoaded && !instanceId && !supportsChat;

  usePageTitle(`Chat - ${workflowName}`);

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
  } = useChatStream(workflowId ?? '');

  // Guard against StrictMode double-mount and dependency-triggered re-runs
  const initRef = useRef(false);

  // Initialize or resume chat on mount
  useEffect(() => {
    if (!workflowId) return;
    // Nothing starts until the workflow has answered — a new session on a
    // workflow that never waits allocated an instance for a chat that could
    // never reply.
    if (!isWorkflowLoaded || isDeadEnd) return;
    if (initRef.current) return;
    initRef.current = true;

    const store = useChatStore.getState();

    if (instanceId) {
      store.resumeChat(workflowId, workflowName, instanceId);

      // Fetch instance detail to get sessionId from inputs.data.sessionId,
      // and load chat history in parallel
      Promise.all([
        getWorkflowInstance(token, workflowId, instanceId),
        fetchChatHistory(token, workflowId, instanceId),
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
                Record<string, unknown> | undefined,
              toolName: waitEvent.data.tool_name as string | undefined,
            });
            store.setStatus('waiting_for_input');
          }

          // Extract sessionId from instance inputs and reconnect
          const sessionId = instanceData?.inputs?.data?.sessionId as
            string | undefined;
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
      store.initChat(workflowId, workflowName);
      // Start a new session — AI agent initiates the conversation
      startSession();
    }

    return () => {
      initRef.current = false;
      cancelStream();
      useChatStore.getState().resetChat();
    };
  }, [workflowId, instanceId, isWorkflowLoaded, isDeadEnd]); // eslint-disable-line react-hooks/exhaustive-deps

  const handleBack = useCallback(() => {
    navigate(`/workflows/${workflowId}`);
  }, [navigate, workflowId]);

  return (
    <div className="flex h-dvh flex-col overflow-hidden">
      {/* Header */}
      <div className="flex items-center gap-3 border-b px-4 py-3">
        <Button
          variant="secondary"
          size="icon"
          onClick={handleBack}
          className="size-8"
        >
          <ArrowLeft className="size-4" />
        </Button>
        <div className="min-w-0 flex-1">
          <h1 className="truncate text-sm font-semibold">{workflowName}</h1>
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

      {isDeadEnd ? (
        <div className="flex flex-1 flex-col items-center justify-center px-4 text-center">
          <MessageSquareOff className="mb-3 size-10 text-muted-foreground/40" />
          <p className="text-sm font-medium text-foreground">
            This workflow does not support chat
          </p>
          <p className="mt-1 max-w-md text-xs text-muted-foreground">
            A conversation needs a step that waits for your reply. Add a Wait
            for signal step — on its own, or as an AI Agent tool — and this
            workflow will start accepting messages.
          </p>
          <Button variant="secondary" className="mt-4" onClick={handleBack}>
            Open workflow
          </Button>
        </div>
      ) : (
        <>
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
        </>
      )}
    </div>
  );
}
