import { useValidationStore } from '../../stores/validationStore';
import { ValidationFilter } from '../../types/validation';
import { cn } from '@/lib/utils';

/**
 * Filter tabs for the validation panel.
 * Allows filtering by All / Errors / Warnings.
 */
export function ValidationFilterTabs() {
  const activeFilter = useValidationStore((s) => s.activeFilter);
  const setActiveFilter = useValidationStore((s) => s.setActiveFilter);
  const errorCount = useValidationStore((s) => s.getErrorCount());
  const warningCount = useValidationStore((s) => s.getWarningCount());

  const totalCount = errorCount + warningCount;

  const tabs: { filter: ValidationFilter; label: string; count: number }[] = [
    { filter: 'all', label: 'All', count: totalCount },
    { filter: 'errors', label: 'Errors', count: errorCount },
    { filter: 'warnings', label: 'Warnings', count: warningCount },
  ];

  return (
    <div className="flex gap-1 border-b bg-muted/20 px-4 py-2">
      {tabs.map(({ filter, label, count }) => (
        <button
          key={filter}
          type="button"
          onClick={() => setActiveFilter(filter)}
          className={cn(
            'rounded-md px-3 py-1 text-xs font-medium transition-colors',
            activeFilter === filter
              ? 'bg-background text-foreground shadow-sm'
              : 'text-muted-foreground hover:bg-muted/50 hover:text-foreground'
          )}
        >
          {label} ({count})
        </button>
      ))}
    </div>
  );
}
