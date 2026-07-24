import { type ReactNode } from 'react';
import { cn } from '@/lib/utils';
import { Spinner } from '@/shared/components/ui/spinner';

export type StatusTone = 'success' | 'warning' | 'error' | 'info' | 'neutral';

/**
 * Tone recipes derived from the theme status tokens (success/warning/
 * destructive/info) so pills follow the theme in both modes with no dark:
 * twins. Exported for the rare case that needs the raw classes on a
 * different shell (e.g. a clickable pill) — never copy these strings.
 */
export const TONE_CLASSES: Record<StatusTone, { pill: string; dot: string }> = {
  success: {
    pill: 'text-success bg-success/10 border-success/30',
    dot: 'bg-success',
  },
  warning: {
    pill: 'text-warning bg-warning/10 border-warning/30',
    dot: 'bg-warning',
  },
  error: {
    pill: 'text-destructive bg-destructive/10 border-destructive/30',
    dot: 'bg-destructive',
  },
  info: {
    pill: 'text-info bg-info/10 border-info/30',
    dot: 'bg-info',
  },
  neutral: {
    pill: 'text-muted-foreground bg-muted border-border',
    dot: 'bg-muted-foreground',
  },
};

/** Pill + dot classes for a tone — use with your own shell element. */
export function statusToneClasses(tone: StatusTone): {
  pill: string;
  dot: string;
} {
  return TONE_CLASSES[tone];
}

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
        <Spinner className="h-3 w-3" />
      ) : dot ? (
        <span
          className={cn(
            'h-1.5 w-1.5 rounded-full',
            t.dot,
            pulse && 'animate-pulse'
          )}
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
