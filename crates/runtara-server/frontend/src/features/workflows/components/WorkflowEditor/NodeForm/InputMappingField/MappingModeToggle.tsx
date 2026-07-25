/**
 * MappingModeToggle - the segmented mode switcher shared by the mapping
 * editors (reference/build in the array and object editors, composite
 * object/array in the composite editor).
 *
 * Owns the segment class strings once so the editors don't hand-roll them.
 */

import type { LucideIcon } from 'lucide-react';
import { cn } from '@/lib/utils';

type MappingModeTone = 'success' | 'info';

const SEGMENT_BASE =
  'flex flex-1 items-center justify-center gap-1.5 rounded-md border px-3 py-2 text-sm transition-colors';
const SEGMENT_INACTIVE =
  'border-input bg-background text-muted-foreground hover:bg-muted/50';
const SEGMENT_ACTIVE: Record<MappingModeTone, string> = {
  success: 'border-success/40 bg-success/10 text-success',
  info: 'border-info/40 bg-info/10 text-info',
};

export interface MappingModeToggleOption<Mode extends string = string> {
  value: Mode;
  label: string;
  icon: LucideIcon;
  /** Optional helper line rendered under the label. */
  description?: string;
  /** Active tint. Defaults to 'success'. */
  tone?: MappingModeTone;
}

interface MappingModeToggleProps<Mode extends string> {
  options: readonly MappingModeToggleOption<Mode>[];
  value: Mode;
  onChange: (mode: Mode) => void;
  /** Disables every segment. */
  disabled?: boolean;
  /** Extra classes for the segment row (e.g. flex-1 or a gap override). */
  className?: string;
}

export function MappingModeToggle<Mode extends string>({
  options,
  value,
  onChange,
  disabled = false,
  className,
}: MappingModeToggleProps<Mode>) {
  return (
    <div className={cn('flex gap-1', className)}>
      {options.map((option) => {
        const Icon = option.icon;
        const isActive = option.value === value;
        return (
          <button
            key={option.value}
            type="button"
            onClick={() => onChange(option.value)}
            disabled={disabled}
            className={cn(
              SEGMENT_BASE,
              isActive
                ? SEGMENT_ACTIVE[option.tone ?? 'success']
                : SEGMENT_INACTIVE,
              disabled && 'cursor-not-allowed opacity-50'
            )}
          >
            <Icon className="size-4" />
            {option.description ? (
              <span className="flex flex-col items-start text-left">
                <span>{option.label}</span>
                <span className="text-xs text-muted-foreground">
                  {option.description}
                </span>
              </span>
            ) : (
              option.label
            )}
          </button>
        );
      })}
    </div>
  );
}
