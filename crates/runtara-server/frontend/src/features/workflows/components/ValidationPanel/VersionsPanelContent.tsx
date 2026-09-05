import { useMemo } from 'react';
import { Check, RefreshCw } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { Badge } from '@/shared/components/ui/badge';
import { SectionLabel } from '@/shared/components/section-label';
import { cn, formatRelativeTime } from '@/lib/utils';
import { WorkflowVersionInfoDto } from '@/features/workflows/queries';

interface VersionsPanelContentProps {
  versions: WorkflowVersionInfoDto[];
  selectedVersion?: number;
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

/** Relative time for a version row, blank when the timestamp is missing. */
function getRelativeTime(dateString?: string): string {
  return dateString ? formatRelativeTime(dateString) : '';
}

/**
 * Panel content showing all workflow versions with controls.
 * Uses a grid layout similar to the History tab.
 */
export function VersionsPanelContent({
  versions,
  selectedVersion,
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
    <div className="flex min-h-0 flex-1 overflow-hidden">
      {/* Versions list */}
      <div className="flex flex-1 flex-col overflow-hidden">
        <div className="flex items-center justify-between border-b bg-muted/20 px-3 py-1.5">
          <SectionLabel as="span">Workflow Versions</SectionLabel>
          <span className="text-xs text-muted-foreground">
            {versions.length} version{versions.length !== 1 ? 's' : ''}
          </span>
        </div>
        <div className="flex-1 overflow-y-auto">
          {sortedVersions.map((version) => {
            // Trust the server's own answer. It resolves the active version as
            // `current_version` falling back to `latest_version`, so a freshly
            // saved version can become the one that executes without anything
            // calling set_current_version. Comparing against the separately
            // cached `currentVersionNumber` — which the save path deliberately
            // leaves untouched — put the "Active" badge on the previous
            // version while runs were already going to the new one.
            const isActive = version.isActive;
            const isSelected = selectedVersion === version.versionNumber;
            // The DB-backed `compiled` flag flips false while a rebuild is
            // mid-flight (the handler deletes the row before the worker
            // writes the new one). Anchoring purely off `compiled` would
            // make the row briefly show "Not compiled" and hide the
            // Rebuild button. Treat an in-flight rebuild of this version
            // as still-compiled for layout purposes — the user clicked
            // the button on a compiled row and shouldn't see it vanish.
            const isRebuilding = rebuildingVersion === version.versionNumber;
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
                  'flex cursor-pointer items-center justify-between border-b px-3 py-2 transition-colors',
                  'hover:bg-muted/50',
                  isSelected && 'border-l-2 border-l-primary bg-accent'
                )}
                onClick={() => {
                  if (version.versionNumber && !isLoading) {
                    onVersionChange(version.versionNumber);
                  }
                }}
              >
                {/* Left side: Version info */}
                <div className="flex items-center gap-3">
                  <span className="min-w-[32px] text-sm font-semibold">
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
                      'h-4 px-1.5 py-0 text-3xs',
                      isRebuilding
                        ? 'border-info bg-info/10 text-info'
                        : version.compiled
                          ? 'border-success bg-success/10 text-success'
                          : isFailed
                            ? 'border-destructive/60 bg-destructive/5 text-destructive'
                            : 'border-warning bg-warning/10 text-warning'
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
                      className="text-3xs tabular-nums text-muted-foreground"
                      title={`Binary: ${formatBytes(version.wasmSize)} · Package source: ${formatBytes(version.packageSize)}`}
                    >
                      wasm {formatBytes(version.wasmSize)} · pkg{' '}
                      {formatBytes(version.packageSize)}
                    </span>
                  )}
                </div>

                {/* Right side: Controls */}
                <div
                  className="flex flex-shrink-0 items-center gap-2"
                  onClick={(e) => e.stopPropagation()}
                >
                  {/* Build button — offered for EVERY version, in all four
                       badge states. Gating it on compiled-or-failed left the
                       amber "Not compiled" row (no compilation record at all:
                       never built, or a rebuild that invalidated the row and
                       then never landed) with no way to build it from the UI
                       at all — the state that most needs the button was the
                       one state that did not get it.
                       The disabled state + spinner provide the debounce —
                       no double-rebuilds. */}
                  {onVersionRebuild && (
                    <Button
                      variant="secondary"
                      bordered
                      size="sm"
                      className="h-6 gap-1 px-2.5 text-3xs"
                      onClick={() => {
                        if (version.versionNumber) {
                          onVersionRebuild(version.versionNumber);
                        }
                      }}
                      disabled={isLoading || isRebuilding}
                      title={
                        isCompiled || isFailed
                          ? 'Force a fresh rebuild of this version'
                          : 'Compile this version'
                      }
                    >
                      <RefreshCw
                        className={cn('size-3', isRebuilding && 'animate-spin')}
                      />
                      <span>
                        {isRebuilding
                          ? 'Rebuilding'
                          : isCompiled || isFailed
                            ? 'Rebuild'
                            : 'Compile'}
                      </span>
                    </Button>
                  )}

                  {/* Activate button */}
                  <Button
                    variant={isActive ? 'secondary' : 'primary'}
                    bordered={isActive}
                    size="sm"
                    className={cn(
                      'h-6 gap-1 px-2.5 text-3xs',
                      isActive &&
                        'border-success/30 bg-success/10 text-success hover:bg-success/10'
                    )}
                    onClick={() => {
                      if (!isActive && version.versionNumber) {
                        onVersionActivate(version.versionNumber);
                      }
                    }}
                    disabled={isLoading || isActive}
                  >
                    <Check className="size-3" />
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
