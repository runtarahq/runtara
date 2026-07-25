import { AlertTriangle } from 'lucide-react';

import { Button } from '@/shared/components/ui/button';
import { Spinner } from '@/shared/components/ui/spinner';

export function ConnectionConflictNotice({
  message,
  loadingLatest,
  changedFields,
  canRecover,
  applying,
  onReload,
  onReapply,
}: {
  message: string;
  loadingLatest: boolean;
  changedFields: readonly string[];
  canRecover: boolean;
  applying: boolean;
  onReload: () => void;
  onReapply: () => void;
}) {
  return (
    <div
      className="mb-6 rounded-lg border border-warning/40 bg-warning/10 p-4 text-foreground"
      role="alert"
    >
      <div className="flex items-start gap-3">
        <AlertTriangle className="mt-0.5 size-5 shrink-0" />
        <div className="min-w-0 flex-1 space-y-2">
          <div>
            <p className="font-medium">Review newer connection changes</p>
            <p className="text-sm">{message}</p>
          </div>
          {loadingLatest ? (
            <p className="flex items-center gap-2 text-sm">
              <Spinner className="size-4" /> Loading the latest version…
            </p>
          ) : (
            <p className="text-sm">
              {changedFields.length > 0
                ? `Changed on the server: ${changedFields.join(', ')}.`
                : 'The server version changed in fields that are not readable in this editor.'}{' '}
              Your submitted draft is still in the form.
            </p>
          )}
          <div className="flex flex-wrap gap-2">
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={!canRecover}
              onClick={onReload}
            >
              Reload latest
            </Button>
            <Button
              type="button"
              size="sm"
              disabled={!canRecover || applying}
              onClick={onReapply}
            >
              Apply my submitted changes
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}
