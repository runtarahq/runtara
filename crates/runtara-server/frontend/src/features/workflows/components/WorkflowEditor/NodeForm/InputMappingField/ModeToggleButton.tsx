import { Icons } from '@/shared/components/icons.tsx';
import { cn } from '@/lib/utils';
import { TOGGLE_SIZE_CLASS } from './value-cell-layout';

export type ValueMode = 'immediate' | 'reference' | 'template' | 'composite';

interface ModeToggleButtonProps {
  mode: ValueMode;
  /**
   * Where a click actually goes. Consumers may restrict the cycle (a Switch
   * case output has no template mode), and a button that names a mode it will
   * not switch to is worse than no label at all.
   */
  nextMode?: ValueMode;
  onClick: () => void;
  disabled?: boolean;
  className?: string;
}

/** Display name for a mode, used to describe where the toggle leads. */
const MODE_NAME: Record<ValueMode, string> = {
  immediate: 'immediate',
  template: 'template',
  reference: 'reference',
  composite: 'composite',
};

const MODE_CONFIG: Record<
  ValueMode,
  {
    icon: keyof typeof Icons;
    title: string;
    ariaLabel: string;
    activeClass: string;
  }
> = {
  immediate: {
    icon: 'type',
    title: 'Immediate Mode - Click to switch to Template mode',
    ariaLabel: 'Switch to template mode',
    activeClass:
      'bg-transparent border-input text-muted-foreground hover:text-foreground',
  },
  template: {
    icon: 'code',
    title: 'Template Mode - Click to switch to Reference mode',
    ariaLabel: 'Switch to reference mode',
    activeClass:
      'bg-purple-100 border-purple-400 text-purple-700 dark:bg-purple-950 dark:border-purple-600 dark:text-purple-300',
  },
  reference: {
    icon: 'gitBranch',
    title: 'Reference Mode - Click to switch to Composite mode',
    ariaLabel: 'Switch to composite mode',
    activeClass:
      'bg-cyan-100 border-cyan-400 text-cyan-700 dark:bg-cyan-950 dark:border-cyan-600 dark:text-cyan-300',
  },
  composite: {
    icon: 'braces',
    title: 'Composite Mode - Click to switch to Immediate mode',
    ariaLabel: 'Switch to immediate mode',
    activeClass:
      'bg-green-100 border-green-400 text-green-700 dark:bg-green-950 dark:border-green-600 dark:text-green-300',
  },
};

/**
 * Single toggle button that cycles through: Immediate → Template → Reference → Composite → Immediate
 */
export function ModeToggleButton({
  mode,
  nextMode,
  onClick,
  disabled = false,
  className,
}: ModeToggleButtonProps) {
  const config = MODE_CONFIG[mode];
  const IconComponent = Icons[config.icon];
  const currentName = MODE_NAME[mode];
  const ariaLabel = nextMode
    ? `Switch to ${MODE_NAME[nextMode]} mode`
    : config.ariaLabel;
  const title = nextMode
    ? `${currentName.charAt(0).toUpperCase()}${currentName.slice(1)} mode — click to switch to ${MODE_NAME[nextMode]} mode`
    : config.title;

  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className={cn(
        // Size comes from value-cell-layout so the gutter that stands in for
        // this button, on rows that have none, reserves the right amount.
        `flex ${TOGGLE_SIZE_CLASS} shrink-0 items-center justify-center rounded-md border transition-colors`,
        'hover:bg-accent focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring',
        config.activeClass,
        disabled && 'cursor-not-allowed opacity-50',
        className
      )}
      aria-label={ariaLabel}
      title={title}
    >
      <IconComponent className="size-4" />
    </button>
  );
}
