import type { ButtonHTMLAttributes, ReactNode } from 'react';
import { cn } from '@/lib/utils';

/**
 * Shared row primitives for picker dialogs (variable picker, condition
 * editor, and friends): the selectable row button, the mono type chip, and
 * the empty-state placeholder. Keeps every picker list visually identical.
 */

interface PickerItemProps extends Omit<
  ButtonHTMLAttributes<HTMLButtonElement>,
  'onSelect'
> {
  /** Invoked when the row is activated (click or keyboard). */
  onSelect: () => void;
  /** Optional leading icon, rendered outside the truncating label area. */
  icon?: ReactNode;
  /** Row content; wrapped in a min-w-0 flex-1 container so it truncates. */
  label: ReactNode;
  /** Optional trailing chip (usually a PickerTypeChip). */
  typeChip?: ReactNode;
}

export function PickerItem({
  onSelect,
  icon,
  label,
  typeChip,
  className,
  ...buttonProps
}: PickerItemProps) {
  return (
    <button
      type="button"
      {...buttonProps}
      onClick={onSelect}
      className={cn(
        'flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-muted-foreground transition-colors hover:bg-accent hover:text-foreground',
        className
      )}
    >
      {icon}
      <div className="min-w-0 flex-1">{label}</div>
      {typeChip}
    </button>
  );
}

/** Mono type chip shown at the trailing edge of a picker row. */
export function PickerTypeChip({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <span
      className={cn(
        'shrink-0 rounded-full bg-black/5 px-1.5 py-0.5 font-mono text-2xs text-muted-foreground dark:bg-white/10',
        className
      )}
    >
      {children}
    </span>
  );
}

/** Centered muted placeholder for an empty (or loading) picker list. */
export function PickerEmpty({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <div className={cn('py-8 text-center text-muted-foreground', className)}>
      {children}
    </div>
  );
}
