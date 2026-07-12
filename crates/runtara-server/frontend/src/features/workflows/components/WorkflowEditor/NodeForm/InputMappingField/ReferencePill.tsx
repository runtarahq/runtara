import { Icons } from '@/shared/components/icons.tsx';
import { cn } from '@/lib/utils';

interface ReferencePillProps {
  path: string;
  /**
   * Resolved type of the referenced value (see NodeForm/reference-type.ts).
   * When absent the type is unknown or runtime-dependent — no badge, neutral
   * icon. Never guessed from the path text.
   */
  type?: string;
  /** Optional step name to display instead of step ID */
  stepName?: string;
  /** Optional field path (without the steps['id'].outputs prefix) */
  fieldPath?: string;
  onRemove: () => void;
  disabled?: boolean;
  className?: string;
}

/**
 * Icon for a resolved type. Unknown types get the neutral reference icon —
 * an icon guessed from path substrings looks confident but lies.
 */
function getIconForType(type?: string) {
  const lowerType = type?.toLowerCase() || '';

  if (lowerType.includes('string') || lowerType.includes('text')) {
    return <Icons.type className="h-3 w-3" />;
  }
  if (
    lowerType.includes('number') ||
    lowerType.includes('int') ||
    lowerType.includes('double') ||
    lowerType.includes('float')
  ) {
    return <Icons.hash className="h-3 w-3" />;
  }
  if (lowerType.includes('boolean') || lowerType.includes('bool')) {
    return <Icons.squareCheck className="h-3 w-3" />;
  }
  if (lowerType.includes('array') || lowerType.includes('list')) {
    return <Icons.list className="h-3 w-3" />;
  }
  if (lowerType.includes('object')) {
    return <Icons.braces className="h-3 w-3" />;
  }
  if (lowerType.includes('date') || lowerType.includes('time')) {
    return <Icons.calendar className="h-3 w-3" />;
  }

  // Unknown / runtime-dependent
  return <Icons.gitBranch className="h-3 w-3" />;
}

/**
 * Displays a reference value as a styled pill/badge
 */
export function ReferencePill({
  path,
  type,
  stepName,
  fieldPath,
  onRemove,
  disabled = false,
  className,
}: ReferencePillProps) {
  // Determine what to display - prefer step name + field path over raw path
  const hasStepInfo = stepName && fieldPath !== undefined;

  return (
    <div
      className={cn(
        'inline-flex items-center gap-1.5 px-2 py-1 rounded-full text-xs',
        'bg-blue-50 text-blue-600 border border-blue-200',
        'dark:bg-blue-950 dark:text-blue-400 dark:border-blue-800',
        disabled && 'opacity-50',
        className
      )}
      title={type ? `${path} — ${type}` : path}
    >
      {getIconForType(type)}
      <span className="truncate max-w-[200px]">
        {hasStepInfo ? (
          <>
            <span className="font-medium">{stepName}</span>
            {fieldPath && (
              <span className="opacity-70">
                {' → '}
                <span className="font-mono">{fieldPath}</span>
              </span>
            )}
          </>
        ) : (
          <span className="font-mono">{path}</span>
        )}
      </span>
      {type && (
        <span className="shrink-0 font-mono text-[10px] leading-none px-1 py-0.5 rounded bg-blue-100 text-blue-700 dark:bg-blue-900 dark:text-blue-300">
          {type}
        </span>
      )}
      {!disabled && (
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            onRemove();
          }}
          className="ml-0.5 p-0.5 rounded-full hover:bg-blue-100 dark:hover:bg-blue-900 transition-colors"
          aria-label="Remove reference"
        >
          <Icons.x className="h-3 w-3" />
        </button>
      )}
    </div>
  );
}
