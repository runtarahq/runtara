import {
  Check,
  ChevronLeft,
  ChevronRight,
  Circle,
  CircleDot,
  Lock,
} from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { Tabs, TabsList, TabsTrigger } from '@/shared/components/ui/tabs';
import { cn } from '@/lib/utils';
import {
  ReportDefinition,
  ReportInteractionOptions,
  ReportViewNavigationState,
} from '../types';
import { humanizeFieldName } from '../utils';

type ReportViewNavigationProps = {
  definition: ReportDefinition;
  navigation?: ReportViewNavigationState | null;
  activeViewId?: string | null;
  onNavigateView?: (
    viewId: string | null,
    options?: Omit<ReportInteractionOptions, 'viewId'>
  ) => void;
};

export function ReportViewNavigation({
  definition,
  navigation,
  activeViewId,
  onNavigateView,
}: ReportViewNavigationProps) {
  const groupState = navigation?.group;
  const group = (definition.viewGroups ?? []).find(
    (candidate) => candidate.id === groupState?.id
  );
  if (!group || !groupState || !activeViewId) return null;

  const viewById = new Map(
    (definition.views ?? []).map((view) => [view.id, view])
  );
  const labelFor = (viewId: string) =>
    viewById.get(viewId)?.title || humanizeFieldName(viewId);

  if (group.mode === 'tabs') {
    return (
      <nav
        aria-label="Report detail views"
        className="report-print-hidden mb-5 overflow-x-auto"
      >
        <Tabs
          value={activeViewId}
          onValueChange={(viewId) =>
            onNavigateView?.(viewId, { replace: false })
          }
        >
          <TabsList className="w-max min-w-full justify-start">
            {(group.viewIds ?? []).map((viewId) => (
              <TabsTrigger key={viewId} value={viewId}>
                {labelFor(viewId)}
              </TabsTrigger>
            ))}
          </TabsList>
        </Tabs>
      </nav>
    );
  }

  const stages = group.stages ?? [];
  const accessible = new Set(groupState.accessibleViewIds ?? []);
  const currentIndex = stages.findIndex(
    (stage) => stage.viewId === groupState.currentViewId
  );
  const activeIndex = stages.findIndex(
    (stage) => stage.viewId === activeViewId
  );
  const previous = activeIndex > 0 ? stages[activeIndex - 1] : undefined;
  const next = activeIndex >= 0 ? stages[activeIndex + 1] : undefined;

  return (
    <nav
      aria-label="Report stages"
      className="report-print-hidden mb-5 rounded-lg border bg-muted/10 p-3"
    >
      <ol className="flex min-w-max items-stretch overflow-x-auto pb-1">
        {stages.map((stage, index) => {
          const isCurrent = index === currentIndex;
          const isComplete = currentIndex >= 0 && index < currentIndex;
          const isSelected = stage.viewId === activeViewId;
          const isAccessible = accessible.has(stage.viewId);
          const Icon = isComplete
            ? Check
            : isCurrent
              ? CircleDot
              : isAccessible
                ? Circle
                : Lock;
          const stateLabel = isCurrent
            ? 'Current'
            : isComplete
              ? 'Completed'
              : isAccessible
                ? 'Available'
                : 'Locked';

          return (
            <li
              key={stage.viewId}
              className={cn(
                'relative flex min-w-36 flex-1 items-stretch',
                index < stages.length - 1 &&
                  'after:absolute after:left-[calc(50%+1.25rem)] after:right-[calc(-50%+1.25rem)] after:top-4 after:h-px after:bg-border'
              )}
            >
              <button
                type="button"
                disabled={!isAccessible}
                aria-current={isCurrent ? 'step' : undefined}
                aria-pressed={isSelected}
                onClick={() =>
                  onNavigateView?.(stage.viewId, { replace: false })
                }
                className={cn(
                  'relative z-10 flex w-full flex-col items-center gap-1 rounded-md px-3 py-1.5 text-center transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2',
                  isAccessible
                    ? 'text-foreground hover:bg-muted/60'
                    : 'cursor-not-allowed text-muted-foreground/60',
                  isSelected && 'bg-background shadow-sm ring-1 ring-border'
                )}
              >
                <span
                  className={cn(
                    'grid h-8 w-8 place-items-center rounded-full border bg-background',
                    (isCurrent || isComplete) &&
                      'border-primary bg-primary text-primary-foreground'
                  )}
                >
                  <Icon className="h-4 w-4" aria-hidden="true" />
                </span>
                <span className="text-sm font-medium">
                  {labelFor(stage.viewId)}
                </span>
                <span className="text-xs text-muted-foreground">
                  {stateLabel}
                </span>
              </button>
            </li>
          );
        })}
      </ol>

      {group.showPreviousNext && (
        <div className="mt-3 flex items-center justify-between border-t pt-3">
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={!previous || !accessible.has(previous.viewId)}
            onClick={() =>
              previous && onNavigateView?.(previous.viewId, { replace: false })
            }
          >
            <ChevronLeft className="mr-1 h-4 w-4" /> Previous stage
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={!next || !accessible.has(next.viewId)}
            onClick={() =>
              next && onNavigateView?.(next.viewId, { replace: false })
            }
          >
            Next stage <ChevronRight className="ml-1 h-4 w-4" />
          </Button>
        </div>
      )}
    </nav>
  );
}
