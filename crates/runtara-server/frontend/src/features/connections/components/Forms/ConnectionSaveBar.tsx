import { Save } from 'lucide-react';

import { Button } from '@/shared/components/ui/button';
import { Spinner } from '@/shared/components/ui/spinner';

type ConnectionSaveBarProps = {
  isLoading?: boolean;
  isSubmitDisabled?: boolean;
  submitLabel: string;
  loadingLabel?: string;
  /** Count of dirty top-level form fields. */
  dirtyCount: number;
  /** Count of stored secrets staged to be cleared on save. */
  clearedCount: number;
  showDiscard: boolean;
  onDiscard: () => void;
};

function changesSummary(dirtyCount: number, clearedCount: number): string {
  const parts: string[] = [];
  if (dirtyCount > 0) {
    parts.push(`${dirtyCount} unsaved change${dirtyCount === 1 ? '' : 's'}`);
  }
  if (clearedCount > 0) {
    parts.push(
      `${clearedCount} secret${clearedCount === 1 ? '' : 's'} will be cleared`
    );
  }
  return parts.join(' · ');
}

/** Sticky bottom action bar owning dirty state and submission. */
export function ConnectionSaveBar({
  isLoading,
  isSubmitDisabled,
  submitLabel,
  loadingLabel,
  dirtyCount,
  clearedCount,
  showDiscard,
  onDiscard,
}: ConnectionSaveBarProps) {
  const summary = changesSummary(dirtyCount, clearedCount);

  return (
    <div className="sticky bottom-0 z-10 border-t border-border bg-muted/50 backdrop-blur-sm dark:bg-background/80">
      <div className="mx-auto flex w-full max-w-2xl items-center gap-3 px-4 py-3 sm:px-6">
        {summary && (
          <div className="flex min-w-0 items-center gap-2 text-sm text-muted-foreground">
            <span
              className="h-1.5 w-1.5 flex-shrink-0 rounded-full bg-warning"
              aria-hidden
            />
            <span className="truncate">{summary}</span>
          </div>
        )}
        <div className="ml-auto flex flex-shrink-0 items-center gap-2">
          {showDiscard && (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={onDiscard}
              disabled={isLoading}
              className="text-muted-foreground hover:text-foreground"
            >
              Discard
            </Button>
          )}
          <Button
            type="submit"
            size="sm"
            disabled={isLoading || isSubmitDisabled}
            className="shadow-sm shadow-primary/20"
          >
            {isLoading ? (
              <>
                <Spinner className="mr-1.5 h-4 w-4" />
                {loadingLabel || 'Saving...'}
              </>
            ) : (
              <>
                <Save className="mr-1.5 h-4 w-4" />
                {submitLabel}
              </>
            )}
          </Button>
        </div>
      </div>
    </div>
  );
}
