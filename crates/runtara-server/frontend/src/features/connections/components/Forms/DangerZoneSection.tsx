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
    <section className="rounded-lg border border-destructive/30 bg-card px-4 py-4">
      <h3 className="font-medium text-destructive">Danger zone</h3>
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
          className="flex-shrink-0 border-destructive/40 text-destructive hover:bg-destructive/10 hover:text-destructive"
        >
          Delete…
        </Button>
      </div>
    </section>
  );
}
