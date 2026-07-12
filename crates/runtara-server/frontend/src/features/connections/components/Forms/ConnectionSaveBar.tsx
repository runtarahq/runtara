import { Loader2, Save } from 'lucide-react';

import { Button } from '@/shared/components/ui/button';

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
    parts.push(
      `${dirtyCount} unsaved change${dirtyCount === 1 ? '' : 's'}`
    );
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
    <div className="sticky bottom-0 z-10 border-t border-slate-200/60 bg-slate-50/80 backdrop-blur-sm dark:bg-background/80 dark:border-slate-700/60">
      <div className="mx-auto w-full max-w-2xl px-4 sm:px-6 py-3 flex items-center gap-3">
        {summary && (
          <div className="flex items-center gap-2 min-w-0 text-sm text-slate-600 dark:text-slate-400">
            <span
              className="w-1.5 h-1.5 rounded-full bg-amber-500 flex-shrink-0"
              aria-hidden
            />
            <span className="truncate">{summary}</span>
          </div>
        )}
        <div className="flex items-center gap-2 ml-auto flex-shrink-0">
          {showDiscard && (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={onDiscard}
              disabled={isLoading}
              className="text-slate-600 hover:text-slate-800 dark:text-slate-400 dark:hover:text-slate-200"
            >
              Discard
            </Button>
          )}
          <Button
            type="submit"
            size="sm"
            disabled={isLoading || isSubmitDisabled}
            className="shadow-sm shadow-blue-600/20"
          >
            {isLoading ? (
              <>
                <Loader2 className="w-4 h-4 mr-1.5 animate-spin" />
                {loadingLabel || 'Saving...'}
              </>
            ) : (
              <>
                <Save className="w-4 h-4 mr-1.5" />
                {submitLabel}
              </>
            )}
          </Button>
        </div>
      </div>
    </div>
  );
}
