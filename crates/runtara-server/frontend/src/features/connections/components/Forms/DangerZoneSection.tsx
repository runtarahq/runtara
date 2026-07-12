import { Button } from '@/shared/components/ui/button';

type DangerZoneSectionProps = {
  /** OAuth authorization-code types have a provider grant that is revoked. */
  isOAuth: boolean;
  isDeleting: boolean;
  onRequestDelete: () => void;
};

/** Bottom-of-form destructive zone hosting the guarded Delete action. */
export function DangerZoneSection({
  isOAuth,
  isDeleting,
  onRequestDelete,
}: DangerZoneSectionProps) {
  return (
    <section className="rounded-lg border border-red-200/70 bg-card px-4 py-4 dark:border-red-900/40">
      <h3 className="font-medium text-red-700 dark:text-red-400">Danger zone</h3>
      <div className="mt-3 flex items-center gap-4">
        <div className="min-w-0 flex-1">
          <p className="text-sm font-medium">Delete this connection</p>
          <p className="mt-0.5 text-xs text-muted-foreground">
            {isOAuth
              ? 'Revokes the provider grant and permanently removes stored credentials.'
              : 'Permanently removes this connection and its stored credentials.'}
          </p>
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={onRequestDelete}
          disabled={isDeleting}
          className="flex-shrink-0 border-red-300 text-red-700 hover:bg-red-50 hover:text-red-800 dark:border-red-800 dark:text-red-400 dark:hover:bg-red-900/30"
        >
          Delete…
        </Button>
      </div>
    </section>
  );
}
