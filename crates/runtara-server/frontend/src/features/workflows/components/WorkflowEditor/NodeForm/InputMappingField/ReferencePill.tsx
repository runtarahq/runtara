import { Icons } from '@/shared/components/icons.tsx';
import { cn } from '@/lib/utils';
import { PICKER_TRUNCATE_MAX_WIDTH } from '@/shared/components/picker-dialog';

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
    return <Icons.type className="size-3" />;
  }
  if (
    lowerType.includes('number') ||
    lowerType.includes('int') ||
    lowerType.includes('double') ||
    lowerType.includes('float')
  ) {
    return <Icons.hash className="size-3" />;
  }
  if (lowerType.includes('boolean') || lowerType.includes('bool')) {
    return <Icons.squareCheck className="size-3" />;
  }
  if (lowerType.includes('array') || lowerType.includes('list')) {
    return <Icons.list className="size-3" />;
  }
  if (lowerType.includes('object')) {
    return <Icons.braces className="size-3" />;
  }
  if (lowerType.includes('date') || lowerType.includes('time')) {
    return <Icons.calendar className="size-3" />;
  }

  // Unknown / runtime-dependent
  return <Icons.gitBranch className="size-3" />;
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
        // max-w-full or the pill overflows its container: the label's 200px cap
        // plus the icon, type badge and remove button is wider than a cell in
        // the 520px panel, and an inline-flex will not shrink on its own.
        'inline-flex max-w-full items-center gap-1.5 rounded-full px-2 py-1 text-xs',
        'border border-primary/30 bg-primary/10 text-primary',
        disabled && 'opacity-50',
        className
      )}
      title={type ? `${path} — ${type}` : path}
    >
      {getIconForType(type)}
      <span className={cn(PICKER_TRUNCATE_MAX_WIDTH, 'min-w-0 truncate')}>
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
        <span className="shrink-0 rounded bg-primary/20 px-1 py-0.5 font-mono text-3xs leading-none text-primary">
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
          className="ml-0.5 rounded-full p-0.5 transition-colors hover:bg-primary/20"
          aria-label="Remove reference"
        >
          <Icons.x className="size-3" />
        </button>
      )}
    </div>
  );
}
