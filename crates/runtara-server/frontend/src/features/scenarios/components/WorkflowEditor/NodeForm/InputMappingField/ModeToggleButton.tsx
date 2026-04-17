import { Icons } from '@/shared/components/icons.tsx';
import { cn } from '@/lib/utils';

export type ValueMode = 'immediate' | 'reference' | 'template' | 'composite';

interface ModeToggleButtonProps {
  mode: ValueMode;
  onClick: () => void;
  disabled?: boolean;
  className?: string;
}

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
  onClick,
  disabled = false,
  className,
}: ModeToggleButtonProps) {
  const config = MODE_CONFIG[mode];
  const IconComponent = Icons[config.icon];

  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className={cn(
        'h-9 w-9 shrink-0 flex items-center justify-center rounded-md border transition-colors',
        'hover:bg-accent focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring',
        config.activeClass,
        disabled && 'opacity-50 cursor-not-allowed',
        className
      )}
      aria-label={config.ariaLabel}
      title={config.title}
    >
      <IconComponent className="h-4 w-4" />
    </button>
  );
}
