import { useState } from 'react';
import Markdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import {
  ChevronDown,
  ChevronRight,
  Loader2,
  Wrench,
  Check,
  Brain,
  Database,
  AlertCircle,
} from 'lucide-react';
import { Badge } from '@/shared/components/ui/badge.tsx';
import { cn } from '@/lib/utils.ts';
import { ChatMessage, ChatSSEEvent } from '@/features/workflows/types/chat';

interface ChatBubbleProps {
  message: ChatMessage;
}

function EventDetails({ events }: { events: ChatSSEEvent[] }) {
  const [expanded, setExpanded] = useState(false);

  if (events.length === 0) return null;

  // Build a compact summary of events
  const toolCalls = events.filter(
    (e) => e.type === 'tool_call' || e.type === 'tool_result'
  );
  const llmEvents = events.filter(
    (e) => e.type === 'llm_start' || e.type === 'llm_end'
  );
  const memoryEvents = events.filter((e) => e.type === 'memory_saved');

  return (
    <div className="mt-2 border-t border-border/40 pt-2">
      <button
        onClick={() => setExpanded(!expanded)}
        className="flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground transition-colors"
      >
        {expanded ? (
          <ChevronDown className="h-3 w-3" />
        ) : (
          <ChevronRight className="h-3 w-3" />
        )}
        <span className="flex items-center gap-1.5">
          {llmEvents.length > 0 && (
            <Badge variant="outline" className="gap-1 py-0 text-[10px]">
              <Brain className="h-2.5 w-2.5" />
              LLM
            </Badge>
          )}
          {toolCalls.length > 0 && (
            <Badge variant="outline" className="gap-1 py-0 text-[10px]">
              <Wrench className="h-2.5 w-2.5" />
              {toolCalls.filter((e) => e.type === 'tool_call').length} tool call
              {toolCalls.filter((e) => e.type === 'tool_call').length !== 1
                ? 's'
                : ''}
            </Badge>
          )}
          {memoryEvents.length > 0 && (
            <Badge variant="outline" className="gap-1 py-0 text-[10px]">
              <Database className="h-2.5 w-2.5" />
              Memory saved
            </Badge>
          )}
        </span>
      </button>

      {expanded && (
        <div className="mt-2 space-y-1.5">
          {events.map((event, i) => (
            <EventItem key={i} event={event} />
          ))}
        </div>
      )}
    </div>
  );
}

function EventItem({ event }: { event: ChatSSEEvent }) {
  switch (event.type) {
    case 'tool_call':
      return (
        <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <Wrench className="h-3 w-3 text-blue-500" />
          <span>
            Calling{' '}
            <code className="rounded bg-muted px-1 py-0.5 text-[10px]">
              {(event.data.tool_name as string) ?? 'tool'}
            </code>
          </span>
        </div>
      );
    case 'tool_result': {
      const duration = event.data.duration_ms as number | undefined;
      return (
        <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <Check className="h-3 w-3 text-green-500" />
          <span>
            <code className="rounded bg-muted px-1 py-0.5 text-[10px]">
              {(event.data.tool_name as string) ?? 'tool'}
            </code>
            {duration != null && (
              <span className="ml-1 text-muted-foreground/70">
                ({String(duration)}ms)
              </span>
            )}
          </span>
        </div>
      );
    }
    case 'llm_start': {
      const model = event.data.model as string | undefined;
      return (
        <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <Brain className="h-3 w-3 text-violet-500" />
          <span>
            Thinking
            {model && (
              <span className="ml-1 text-muted-foreground/70">({model})</span>
            )}
          </span>
        </div>
      );
    }
    case 'llm_end':
      return (
        <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <Check className="h-3 w-3 text-violet-500" />
          <span>Done thinking</span>
        </div>
      );
    case 'memory_saved':
      return (
        <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <Database className="h-3 w-3 text-emerald-500" />
          <span>
            Memory saved ({(event.data.message_count as number) ?? '?'}{' '}
            messages)
          </span>
        </div>
      );
    default:
      return null;
  }
}

export function ChatBubble({ message }: ChatBubbleProps) {
  const { role, content, events, isStreaming } = message;

  if (role === 'user') {
    return (
      <div className="flex justify-end">
        <div className="max-w-[80%] rounded-2xl rounded-tr-md bg-primary px-4 py-2.5 text-sm text-primary-foreground">
          <p className="whitespace-pre-wrap">{content}</p>
        </div>
      </div>
    );
  }

  if (role === 'system') {
    return (
      <div className="flex justify-center">
        <div className="flex items-center gap-1.5 rounded-full bg-muted/50 px-3 py-1 text-xs text-muted-foreground">
          {events.some((e) => e.type === 'waiting_for_input') && (
            <AlertCircle className="h-3 w-3 text-amber-500" />
          )}
          <span>{content}</span>
        </div>
      </div>
    );
  }

  // Don't render empty assistant bubbles (placeholder was created but no content arrived)
  if (!content && !isStreaming && events.length === 0) {
    return null;
  }

  // Assistant message
  return (
    <div className="flex justify-start">
      <div
        className={cn(
          'max-w-[80%] rounded-2xl rounded-tl-md border bg-card px-4 py-2.5 text-sm',
          isStreaming && !content && 'animate-pulse'
        )}
      >
        {content ? (
          <div className="prose prose-sm dark:prose-invert max-w-none [&>*:first-child]:mt-0 [&>*:last-child]:mb-0">
            <Markdown remarkPlugins={[remarkGfm]}>{content}</Markdown>
          </div>
        ) : isStreaming ? (
          <div className="flex items-center gap-2 text-muted-foreground">
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
            <span className="text-xs">Thinking...</span>
          </div>
        ) : null}

        {isStreaming && content && (
          <span className="inline-block h-4 w-0.5 animate-pulse bg-foreground/60 ml-0.5" />
        )}

        <EventDetails events={events} />
      </div>
    </div>
  );
}
