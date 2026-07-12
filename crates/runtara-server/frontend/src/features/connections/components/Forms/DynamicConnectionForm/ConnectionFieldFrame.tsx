import { RotateCcw, Trash2 } from 'lucide-react';

import { Button } from '@/shared/components/ui/button';

interface ConnectionFieldFrameProps {
  label: string;
  configured: boolean;
  clearable: boolean;
  cleared: boolean;
  requiresReauthorization?: boolean;
  onClear: () => void;
  onUndoClear: () => void;
}

/** Connection-owned state/actions rendered around a shared field control. */
export function ConnectionFieldFrame({
  label,
  configured,
  clearable,
  cleared,
  requiresReauthorization = false,
  onClear,
  onUndoClear,
}: ConnectionFieldFrameProps) {
  return (
    <div className="flex items-start justify-between gap-3 rounded-md bg-muted/40 px-3 py-2">
      <div className="min-w-0 text-xs">
        <p
          className={
            cleared
              ? 'font-medium text-amber-700 dark:text-amber-400'
              : configured
                ? 'text-emerald-700 dark:text-emerald-400'
                : 'text-muted-foreground'
          }
        >
          {cleared
            ? 'The stored secret will be cleared when you save.'
            : configured
              ? 'A secret is configured. Enter a value only to replace it.'
              : 'No secret is configured.'}
        </p>
        {requiresReauthorization && (
          <p className="mt-1 text-muted-foreground">
            Replacing this value will require reconnecting the provider.
          </p>
        )}
      </div>
      {cleared ? (
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="h-7 shrink-0 px-2 text-xs"
          aria-label={`Undo clearing stored ${label}`}
          onClick={onUndoClear}
        >
          <RotateCcw className="mr-1 h-3.5 w-3.5" />
          Undo
        </Button>
      ) : (
        configured &&
        clearable && (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-7 shrink-0 px-2 text-xs text-destructive hover:text-destructive"
            aria-label={`Clear stored ${label}`}
            onClick={onClear}
          >
            <Trash2 className="mr-1 h-3.5 w-3.5" />
            Clear
          </Button>
        )
      )}
    </div>
  );
}
