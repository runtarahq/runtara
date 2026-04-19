import { Icons } from '@/shared/components/icons.tsx';
import { cn } from '@/lib/utils';

interface ReferencePillProps {
  path: string;
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
 * Get icon based on inferred type from path or explicit type
 */
function getIconForType(type?: string, path?: string) {
  const lowerType = type?.toLowerCase() || '';
  const lowerPath = path?.toLowerCase() || '';

  // Check explicit type first
  if (lowerType.includes('string') || lowerType.includes('text')) {
    return <Icons.type className="h-3 w-3" />;
  }
  if (
    lowerType.includes('number') ||
    lowerType.includes('int') ||
    lowerType.includes('integer') ||
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
  if (
    lowerType.includes('date') ||
    lowerType.includes('time') ||
    lowerPath.includes('date') ||
    lowerPath.includes('time')
  ) {
    return <Icons.calendar className="h-3 w-3" />;
  }

  // Infer from path
  if (lowerPath.includes('email')) {
    return <Icons.mail className="h-3 w-3" />;
  }
  if (lowerPath.includes('name')) {
    return <Icons.user className="h-3 w-3" />;
  }
  if (lowerPath.includes('id') || lowerPath.includes('key')) {
    return <Icons.key className="h-3 w-3" />;
  }
  if (
    lowerPath.includes('price') ||
    lowerPath.includes('amount') ||
    lowerPath.includes('total') ||
    lowerPath.includes('cost')
  ) {
    return <Icons.dollarSign className="h-3 w-3" />;
  }

  // Default icon
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
    >
      {getIconForType(type, path)}
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
