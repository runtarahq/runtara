import { type ReactNode } from 'react';
import { Loader2 } from 'lucide-react';
import { cn } from '@/lib/utils';

export type StatusTone = 'success' | 'warning' | 'error' | 'info' | 'neutral';

const TONE_CLASSES: Record<StatusTone, { pill: string; dot: string }> = {
  success: {
    pill: 'text-emerald-700 bg-emerald-50 border-emerald-200/60 dark:text-emerald-400 dark:bg-emerald-900/30 dark:border-emerald-700/40',
    dot: 'bg-emerald-500 dark:bg-emerald-400',
  },
  warning: {
    pill: 'text-amber-700 bg-amber-50 border-amber-200/60 dark:text-amber-400 dark:bg-amber-900/30 dark:border-amber-700/40',
    dot: 'bg-amber-500 dark:bg-amber-400',
  },
  error: {
    pill: 'text-red-700 bg-red-50 border-red-200/60 dark:text-red-400 dark:bg-red-900/30 dark:border-red-700/40',
    dot: 'bg-red-500 dark:bg-red-400',
  },
  info: {
    pill: 'text-blue-700 bg-blue-50 border-blue-200/60 dark:text-blue-400 dark:bg-blue-900/30 dark:border-blue-700/40',
    dot: 'bg-blue-500 dark:bg-blue-400',
  },
  neutral: {
    pill: 'text-slate-700 bg-slate-100 border-slate-200/60 dark:text-slate-400 dark:bg-slate-800 dark:border-slate-700/40',
    dot: 'bg-slate-500 dark:bg-slate-400',
  },
};

export interface StatusPillProps {
  tone?: StatusTone;
  label: ReactNode;
  /** Show the leading status dot (ignored when `spin` is set). Default true. */
  dot?: boolean;
  /** Replace the dot with a spinner (for in-progress states). */
  spin?: boolean;
  /** Animate the dot (for pending/suspended states). */
  pulse?: boolean;
  className?: string;
}

/**
 * Soft, dotted status pill matching the console mockup. Tone colors mirror the
 * existing light + dark execution badge palette so dark mode keeps working.
 */
export function StatusPill({
  tone = 'neutral',
  label,
  dot = true,
  spin = false,
  pulse = false,
  className,
}: StatusPillProps) {
  const t = TONE_CLASSES[tone];
  return (
    <span
      className={cn(
        'inline-flex items-center justify-center gap-1.5 rounded-full border px-2.5 py-1 text-xs font-medium',
        t.pill,
        className
      )}
    >
      {spin ? (
        <Loader2 className="h-3 w-3 animate-spin" />
      ) : dot ? (
        <span
          className={cn('h-1.5 w-1.5 rounded-full', t.dot, pulse && 'animate-pulse')}
        />
      ) : null}
      {label}
    </span>
  );
}

export interface ExecutionStatusPill {
  tone: StatusTone;
  label: string;
  spin?: boolean;
  pulse?: boolean;
}

/**
 * Maps a workflow execution status string (case-insensitive) to pill props.
 * Centralizes the per-status styling that used to live inline in the
 * invocation-history columns.
 */
export function executionStatusPill(status: string): ExecutionStatusPill {
  switch ((status || '').toLowerCase()) {
    case 'completed':
      return { tone: 'success', label: 'Completed' };
    case 'failed':
      return { tone: 'error', label: 'Failed' };
    case 'timeout':
      return { tone: 'warning', label: 'Timeout' };
    case 'cancelled':
    case 'canceled':
      return { tone: 'neutral', label: 'Cancelled' };
    case 'running':
      return { tone: 'info', label: 'Running', spin: true };
    case 'compiling':
      return { tone: 'info', label: 'Compiling', spin: true };
    case 'queued':
      return { tone: 'neutral', label: 'Queued' };
    case 'suspended':
      return { tone: 'info', label: 'Suspended', pulse: true };
    default:
      return { tone: 'neutral', label: status || 'Unknown' };
  }
}
