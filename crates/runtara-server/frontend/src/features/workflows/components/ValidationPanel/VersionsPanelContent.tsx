import { useMemo } from 'react';
import { Check, RefreshCw } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { Badge } from '@/shared/components/ui/badge';
import { cn } from '@/lib/utils';
import { WorkflowVersionInfoDto } from '@/features/workflows/queries';

interface VersionsPanelContentProps {
  versions: WorkflowVersionInfoDto[];
  selectedVersion?: number;
  currentVersionNumber?: number;
  onVersionChange: (version: number | undefined) => void;
  onVersionActivate: (version: number) => void;
  /** Force-recompile a previously compiled version. The handler invalidates
   *  the DB row and re-enqueues with force_recompile=true, so both DB and
   *  runtime caches miss and a real rebuild happens. */
  onVersionRebuild?: (version: number) => void;
  /** Version currently being rebuilt — disables its Rebuild button and
   *  switches the spinner on so the row reflects the in-flight state. */
  rebuildingVersion?: number;
  isLoading?: boolean;
}

/**
 * Format a byte count for the version list. Compact form ("12.4 KB") fits
 * inline next to the relative-time text without wrapping. Falls back to "—"
 * when the size is missing (pre-existing rows without the column populated).
 */
function formatBytes(bytes?: number | null): string {
  if (bytes === undefined || bytes === null) return '—';
  if (bytes < 1024) return `${bytes} B`;
  const kb = bytes / 1024;
  if (kb < 1024) return `${kb.toFixed(kb < 10 ? 1 : 0)} KB`;
  const mb = kb / 1024;
  return `${mb.toFixed(mb < 10 ? 1 : 0)} MB`;
}

/**
 * Get relative time string (e.g., "2 hours ago", "3 days ago")
 */
function getRelativeTime(dateString?: string): string {
  if (!dateString) return '';

  const now = new Date();
  const past = new Date(dateString);
  const diffMs = now.getTime() - past.getTime();
  const diffMins = Math.floor(diffMs / 60000);
  const diffHours = Math.floor(diffMs / 3600000);
  const diffDays = Math.floor(diffMs / 86400000);

  if (diffMins < 1) return 'just now';
  if (diffMins < 60) return `${diffMins} min ago`;
  if (diffHours < 24) return `${diffHours} hr ago`;
  if (diffDays < 7) return `${diffDays} days ago`;
  return new Date(dateString).toLocaleString();
}

/**
 * Panel content showing all workflow versions with controls.
 * Uses a grid layout similar to the History tab.
 */
export function VersionsPanelContent({
  versions,
  selectedVersion,
  currentVersionNumber,
  onVersionChange,
  onVersionActivate,
  onVersionRebuild,
  rebuildingVersion,
  isLoading = false,
}: VersionsPanelContentProps) {
  const sortedVersions = useMemo(
    () =>
      [...versions].sort(
        (a, b) => (b.versionNumber || 0) - (a.versionNumber || 0)
      ),
    [versions]
  );

  if (versions.length === 0) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
        No versions available
      </div>
    );
  }

  return (
    <div className="flex flex-1 min-h-0 overflow-hidden">
      {/* Versions list */}
      <div className="flex-1 flex flex-col overflow-hidden">
        <div className="flex items-center justify-between px-3 py-1.5 border-b bg-muted/20">
          <span className="text-xs font-medium text-muted-foreground uppercase tracking-wider">
            Workflow Versions
          </span>
          <span className="text-xs text-muted-foreground">
            {versions.length} version{versions.length !== 1 ? 's' : ''}
          </span>
        </div>
        <div className="flex-1 overflow-y-auto">
          {sortedVersions.map((version) => {
            const isActive = currentVersionNumber === version.versionNumber;
            const isSelected = selectedVersion === version.versionNumber;
            // The DB-backed `compiled` flag flips false while a rebuild is
            // mid-flight (the handler deletes the row before the worker
            // writes the new one). Anchoring purely off `compiled` would
            // make the row briefly show "Not compiled" and hide the
            // Rebuild button. Treat an in-flight rebuild of this version
            // as still-compiled for layout purposes — the user clicked
            // the button on a compiled row and shouldn't see it vanish.
            const isRebuilding =
              rebuildingVersion === version.versionNumber;
            const isCompiled = version.compiled || isRebuilding;
            // A row is "failed" when the worker recorded a failure and we
            // are NOT currently mid-rebuild (the rebuild click should
            // visually supersede the prior failure state). Rebuild has to
            // be offered for failed rows too — otherwise the user is
            // stuck after a transient registration error.
            const isFailed =
              !isRebuilding && version.compilationStatus === 'failed';

            return (
              <div
                key={version.versionId ?? version.versionNumber}
                className={cn(
                  'flex items-center justify-between px-3 py-2 border-b cursor-pointer transition-colors',
                  'hover:bg-muted/50',
                  isSelected && 'bg-accent border-l-2 border-l-primary'
                )}
                onClick={() => {
                  if (version.versionNumber && !isLoading) {
                    onVersionChange(version.versionNumber);
                  }
                }}
              >
                {/* Left side: Version info */}
                <div className="flex items-center gap-3">
                  <span className="text-sm font-semibold min-w-[32px]">
                    v{version.versionNumber}
                  </span>
                  <span className="text-xs text-muted-foreground">
                    {getRelativeTime(version.updatedAt)}
                  </span>
                  {/* Compilation status badge. Four-state:
                      - in-flight rebuild → blue "Compiling"
                      - success → green "Compiled"
                      - failed → red "Failed" (tooltip carries the worker error)
                      - pending / never attempted → amber "Not compiled" */}
                  <Badge
                    variant="outline"
                    className={cn(
                      'text-[10px] px-1.5 py-0 h-4',
                      isRebuilding
                        ? 'border-blue-500 text-blue-600 bg-blue-50 dark:bg-blue-950/30'
                        : version.compiled
                          ? 'border-green-500 text-green-600 bg-green-50 dark:bg-green-950/30'
                          : isFailed
                            ? 'border-destructive/60 text-destructive bg-destructive/5'
                            : 'border-amber-500 text-amber-600 bg-amber-50 dark:bg-amber-950/30'
                    )}
                    title={
                      isFailed && version.errorMessage
                        ? version.errorMessage
                        : undefined
                    }
                  >
                    {isRebuilding
                      ? 'Compiling'
                      : version.compiled
                        ? 'Compiled'
                        : isFailed
                          ? 'Failed'
                          : 'Not compiled'}
                  </Badge>
                  {/* Size figures — only meaningful for compiled rows.
                      `wasm` = composed binary, `pkg` = generated crate
                      source (lib.rs + Cargo.toml + WIT + WAC). Stays
                      visible during rebuild (showing the previous build's
                      sizes) rather than blanking out — they remain accurate
                      until the new compile lands. */}
                  {isCompiled && version.wasmSize != null && (
                    <span
                      className="text-[10px] text-muted-foreground tabular-nums"
                      title={`Binary: ${formatBytes(version.wasmSize)} · Package source: ${formatBytes(version.packageSize)}`}
                    >
                      wasm {formatBytes(version.wasmSize)} · pkg{' '}
                      {formatBytes(version.packageSize)}
                    </span>
                  )}
                </div>

                {/* Right side: Controls */}
                <div
                  className="flex items-center gap-2 flex-shrink-0"
                  onClick={(e) => e.stopPropagation()}
                >
                  {/* Rebuild button — visible for:
                       - compiled rows (re-run an already-successful build)
                       - failed rows (retry after a transient error like a
                         missing runtara-environment connection)
                       - rows currently mid-rebuild (so the button doesn't
                         vanish during the brief window where the handler
                         has already invalidated the DB row but the worker
                         hasn't written the new one).
                       The disabled state + spinner provide the debounce —
                       no double-rebuilds. */}
                  {(isCompiled || isFailed) && onVersionRebuild && (
                    <Button
                      variant="outline"
                      size="sm"
                      className="h-6 px-2.5 text-[10px] gap-1"
                      onClick={() => {
                        if (version.versionNumber) {
                          onVersionRebuild(version.versionNumber);
                        }
                      }}
                      disabled={isLoading || isRebuilding}
                      title="Force a fresh rebuild of this version"
                    >
                      <RefreshCw
                        className={cn(
                          'h-3 w-3',
                          isRebuilding && 'animate-spin'
                        )}
                      />
                      <span>
                        {isRebuilding ? 'Rebuilding' : 'Rebuild'}
                      </span>
                    </Button>
                  )}

                  {/* Activate button */}
                  <Button
                    variant={isActive ? 'outline' : 'default'}
                    size="sm"
                    className={cn(
                      'h-6 px-2.5 text-[10px] gap-1',
                      isActive &&
                        'border-green-200 bg-green-50 text-green-700 hover:bg-green-50 dark:border-green-800 dark:bg-green-900/20 dark:text-green-400'
                    )}
                    onClick={() => {
                      if (!isActive && version.versionNumber) {
                        onVersionActivate(version.versionNumber);
                      }
                    }}
                    disabled={isLoading || isActive}
                  >
                    <Check className="h-3 w-3" />
                    <span>{isActive ? 'Active' : 'Activate'}</span>
                  </Button>
                </div>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
