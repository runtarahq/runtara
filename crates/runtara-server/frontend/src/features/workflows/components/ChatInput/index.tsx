import { useState, useEffect, useCallback, useRef, KeyboardEvent } from 'react';
import { Send } from 'lucide-react';
import { Button } from '@/shared/components/ui/button.tsx';
import { Textarea } from '@/shared/components/ui/textarea.tsx';
import {
  ChatStatus,
  WaitingForInputData,
} from '@/features/workflows/types/chat';
import { ChatFormInput } from './ChatFormInput';
import { Spinner } from '@/shared/components/ui/spinner';

interface ChatInputProps {
  onSend: (message: string) => Promise<boolean>;
  onSignalResponse?: (response: string) => Promise<boolean>;
  status: ChatStatus;
  waitingForInput: WaitingForInputData | null;
  pendingInputs: WaitingForInputData[];
  onSelectInput: (input: WaitingForInputData | null) => void;
  onSubmitInput: (
    requestId: string,
    payload: Record<string, unknown>,
    instanceId?: string
  ) => Promise<boolean>;
}

export function ChatInput({
  onSend,
  onSignalResponse,
  status,
  waitingForInput,
  pendingInputs,
  onSelectInput,
  onSubmitInput,
}: ChatInputProps) {
  const [value, setValue] = useState('');
  const [isSending, setIsSending] = useState(false);
  const sendingRef = useRef(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    // A stale reply must not become a draft response to the next request.
    setValue('');
  }, [waitingForInput?.requestId, waitingForInput?.instanceId]);

  const isDisabled = (status === 'streaming' && !waitingForInput) || isSending;
  const isDone = status === 'done';
  const isWaiting = status === 'waiting_for_input';
  const canSend = value.trim().length > 0 && !isDisabled;

  // Detect whether the response schema is a simple conversational input
  // (no schema, empty schema, or a single "message" field).
  // In that case, suppress all "input required" hints for a conversational look.
  const isSimpleMessageSchema = (() => {
    const schema = waitingForInput?.responseSchema;
    if (!schema || typeof schema !== 'object') return true;
    const keys = Object.keys(schema);
    return keys.length === 0 || (keys.length === 1 && keys[0] === 'message');
  })();

  // Only show schema hints when the response schema requires structured input.
  const schemaFieldDescription = (() => {
    if (isSimpleMessageSchema) return undefined;
    const schema = waitingForInput?.responseSchema;
    if (!schema || typeof schema !== 'object') return undefined;
    const fieldEntries = Object.entries(schema);
    if (fieldEntries.length !== 1) return undefined;
    const [, fieldDef] = fieldEntries[0];
    const def = fieldDef as Record<string, unknown> | undefined;
    return typeof def?.description === 'string' && def.description
      ? def.description
      : undefined;
  })();

  const handleSend = useCallback(async () => {
    const trimmed = value.trim();
    if (!trimmed || sendingRef.current) return;
    sendingRef.current = true;
    setIsSending(true);
    try {
      const accepted = await (isWaiting && onSignalResponse
        ? onSignalResponse(trimmed)
        : onSend(trimmed));
      if (accepted) {
        setValue('');
        if (textareaRef.current) textareaRef.current.style.height = 'auto';
      }
    } finally {
      sendingRef.current = false;
      setIsSending(false);
    }
  }, [value, isWaiting, onSend, onSignalResponse]);

  const handleKeyDown = useCallback(
    (e: KeyboardEvent<HTMLTextAreaElement>) => {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        if (canSend) {
          handleSend();
        }
      }
    },
    [canSend, handleSend]
  );

  // Auto-resize textarea
  const handleChange = useCallback(
    (e: React.ChangeEvent<HTMLTextAreaElement>) => {
      setValue(e.target.value);
      const textarea = e.target;
      textarea.style.height = 'auto';
      textarea.style.height = `${Math.min(textarea.scrollHeight, 120)}px`;
    },
    []
  );

  if (pendingInputs.length > 1 && !waitingForInput) {
    return (
      <div className="space-y-2 border-t p-4">
        <p className="text-sm">Choose which request to answer</p>
        {pendingInputs.map((input) => (
          <Button
            key={JSON.stringify([input.instanceId, input.requestId])}
            variant="secondary"
            onClick={() => onSelectInput(input)}
          >
            {input.message || input.toolName || input.requestId}
          </Button>
        ))}
      </div>
    );
  }
  const chooser =
    pendingInputs.length > 1 ? (
      <Button variant="secondary" onClick={() => onSelectInput(null)}>
        Choose another request
      </Button>
    ) : null;
  if (isWaiting && !isSimpleMessageSchema && waitingForInput) {
    return (
      <div>
        {chooser}
        <ChatFormInput
          key={JSON.stringify([
            waitingForInput.instanceId,
            waitingForInput.requestId,
          ])}
          waitingForInput={waitingForInput}
          onSubmit={onSubmitInput}
        />
      </div>
    );
  }

  return (
    <div className="border-t bg-background px-4 py-3">
      {chooser}
      {isWaiting &&
        !isSimpleMessageSchema &&
        (waitingForInput?.message || schemaFieldDescription) && (
          <div className="mb-2 rounded-lg border border-warning/30 bg-warning/10 px-3 py-2 text-xs text-warning">
            {waitingForInput?.message || schemaFieldDescription}
          </div>
        )}

      {isDisabled && (
        <div className="mb-2 flex items-center gap-1.5 text-xs text-muted-foreground">
          <Spinner className="size-3" />
          AI is thinking...
        </div>
      )}

      <div className="flex items-end gap-2">
        <Textarea
          ref={textareaRef}
          value={value}
          onChange={handleChange}
          onKeyDown={handleKeyDown}
          placeholder={
            isDone
              ? 'Chat completed'
              : schemaFieldDescription || 'Type a message...'
          }
          disabled={isDisabled || isDone}
          className="max-h-[120px] min-h-[40px] resize-none"
          rows={1}
        />
        <Button
          onClick={handleSend}
          disabled={!canSend || isDone}
          size="icon"
          className="size-10 shrink-0"
        >
          <Send className="size-4" />
        </Button>
      </div>
    </div>
  );
}
