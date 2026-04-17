import { useState, useCallback, useRef, KeyboardEvent } from 'react';
import { Send, Loader2 } from 'lucide-react';
import { Button } from '@/shared/components/ui/button.tsx';
import { Textarea } from '@/shared/components/ui/textarea.tsx';
import {
  ChatStatus,
  WaitingForInputData,
} from '@/features/scenarios/types/chat';
import { ChatFormInput } from './ChatFormInput';

interface ChatInputProps {
  onSend: (message: string) => void;
  onSignalResponse?: (response: string) => void;
  status: ChatStatus;
  waitingForInput: WaitingForInputData | null;
  instanceId?: string | null;
  token?: string;
}

export function ChatInput({
  onSend,
  onSignalResponse,
  status,
  waitingForInput,
  instanceId,
  token,
}: ChatInputProps) {
  const [value, setValue] = useState('');
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const isDisabled = status === 'streaming';
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

  const handleSend = useCallback(() => {
    const trimmed = value.trim();
    if (!trimmed) return;

    if (isWaiting && onSignalResponse) {
      onSignalResponse(trimmed);
    } else {
      onSend(trimmed);
    }

    setValue('');

    // Reset textarea height
    if (textareaRef.current) {
      textareaRef.current.style.height = 'auto';
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

  // Render structured form when waiting for non-simple schema input
  if (
    isWaiting &&
    !isSimpleMessageSchema &&
    waitingForInput &&
    instanceId &&
    token
  ) {
    return (
      <ChatFormInput
        waitingForInput={waitingForInput}
        instanceId={instanceId}
        token={token}
      />
    );
  }

  return (
    <div className="border-t bg-background px-4 py-3">
      {isWaiting &&
        !isSimpleMessageSchema &&
        (waitingForInput?.message || schemaFieldDescription) && (
          <div className="mb-2 rounded-lg bg-amber-50 dark:bg-amber-900/20 border border-amber-200/60 dark:border-amber-700/40 px-3 py-2 text-xs text-amber-700 dark:text-amber-400">
            {waitingForInput?.message || schemaFieldDescription}
          </div>
        )}

      {isDisabled && (
        <div className="mb-2 flex items-center gap-1.5 text-xs text-muted-foreground">
          <Loader2 className="h-3 w-3 animate-spin" />
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
          className="min-h-[40px] max-h-[120px] resize-none"
          rows={1}
        />
        <Button
          onClick={handleSend}
          disabled={!canSend || isDone}
          size="icon"
          className="h-10 w-10 shrink-0"
        >
          <Send className="h-4 w-4" />
        </Button>
      </div>
    </div>
  );
}
