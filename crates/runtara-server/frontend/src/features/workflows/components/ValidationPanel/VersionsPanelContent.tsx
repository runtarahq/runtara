import { useMemo } from 'react';
import { Check } from 'lucide-react';
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
  isLoading?: boolean;
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
            const isCompiled = version.compiled;

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
                  {/* Compilation status badge */}
                  <Badge
                    variant="outline"
                    className={cn(
                      'text-[10px] px-1.5 py-0 h-4',
                      isCompiled
                        ? 'border-green-500 text-green-600 bg-green-50 dark:bg-green-950/30'
                        : 'border-amber-500 text-amber-600 bg-amber-50 dark:bg-amber-950/30'
                    )}
                  >
                    {isCompiled ? 'Compiled' : 'Not compiled'}
                  </Badge>
                </div>

                {/* Right side: Controls */}
                <div
                  className="flex items-center gap-3 flex-shrink-0"
                  onClick={(e) => e.stopPropagation()}
                >
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
