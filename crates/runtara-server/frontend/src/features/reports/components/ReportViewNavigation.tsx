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
  const showPrevious = group.showPrevious ?? group.showPreviousNext ?? false;
  const showNext = group.showNext ?? group.showPreviousNext ?? false;
  const previousLabel = group.previousLabel?.trim() || 'Previous';
  const nextLabel = group.nextLabel?.trim() || 'Next';
  const isCurrentOnly = group.access === 'current_only';

  return (
    <nav
      aria-label="Report stages"
      className="report-print-hidden mb-6 rounded-xl border bg-card px-4 py-4 shadow-sm"
    >
      <div className="-mx-1 overflow-x-auto px-1 pb-1">
        <ol className="flex min-w-[30rem] items-start">
          {stages.map((stage, index) => {
            const isCurrent = index === currentIndex;
            const isComplete = currentIndex >= 0 && index < currentIndex;
            const isSelected = stage.viewId === activeViewId;
            const isAccessible = accessible.has(stage.viewId);
            const isInteractive = isAccessible && !isCurrentOnly;
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
                  'relative flex min-w-40 flex-1 items-start',
                  index < stages.length - 1 &&
                    'after:absolute after:left-[calc(50%+1.25rem)] after:right-[calc(-50%+1.25rem)] after:top-4 after:h-0.5',
                  index < currentIndex
                    ? 'after:bg-primary/50'
                    : 'after:bg-border'
                )}
              >
                <button
                  type="button"
                  disabled={!isInteractive}
                  aria-current={isCurrent ? 'step' : undefined}
                  aria-pressed={isSelected}
                  onClick={() =>
                    onNavigateView?.(stage.viewId, { replace: false })
                  }
                  className={cn(
                    'group relative z-10 flex w-full flex-col items-center gap-1.5 rounded-lg px-2 py-0.5 text-center focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-4',
                    isInteractive
                      ? 'text-foreground hover:text-primary'
                      : isAccessible
                        ? 'cursor-default text-foreground'
                        : 'cursor-not-allowed text-muted-foreground/55'
                  )}
                >
                  <span
                    className={cn(
                      'grid h-8 w-8 place-items-center rounded-full border bg-background transition-[border-color,box-shadow,color,background-color]',
                      isComplete &&
                        'border-primary bg-primary text-primary-foreground',
                      isCurrent &&
                        'border-2 border-primary text-primary ring-4 ring-primary/10',
                      isInteractive &&
                        !isComplete &&
                        !isCurrent &&
                        'group-hover:border-primary/50 group-hover:text-primary',
                      isSelected &&
                        !isCurrent &&
                        'ring-4 ring-muted-foreground/10'
                    )}
                  >
                    <Icon className="h-4 w-4" aria-hidden="true" />
                  </span>
                  <span
                    className={cn(
                      'text-sm font-medium leading-tight transition-colors',
                      isCurrent && 'text-primary',
                      !isAccessible && 'text-muted-foreground/70'
                    )}
                  >
                    {labelFor(stage.viewId)}
                  </span>
                  <span
                    className={cn(
                      'rounded-full px-2 py-0.5 text-[11px] font-medium leading-none',
                      isCurrent && 'bg-primary/10 text-primary',
                      isComplete && 'bg-muted text-muted-foreground',
                      isAccessible &&
                        !isCurrent &&
                        !isComplete &&
                        'bg-muted text-foreground',
                      !isAccessible && 'text-muted-foreground/70'
                    )}
                  >
                    {isSelected && !isCurrent
                      ? `Viewing · ${stateLabel}`
                      : stateLabel}
                  </span>
                </button>
              </li>
            );
          })}
        </ol>
      </div>

      {(showPrevious || showNext) && (
        <div className="mt-4 grid grid-cols-[1fr_auto_1fr] items-center border-t pt-3">
          {showPrevious ? (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              aria-label={group.previousLabel?.trim() || 'Previous stage'}
              disabled={
                isCurrentOnly || !previous || !accessible.has(previous.viewId)
              }
              onClick={() =>
                previous &&
                onNavigateView?.(previous.viewId, { replace: false })
              }
              className="w-fit justify-self-start px-2 text-muted-foreground hover:text-foreground"
            >
              <ChevronLeft className="mr-1 h-4 w-4" /> {previousLabel}
            </Button>
          ) : (
            <span aria-hidden="true" />
          )}
          <span className="text-xs font-medium tabular-nums text-muted-foreground">
            {activeIndex + 1} of {stages.length}
          </span>
          {showNext ? (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              aria-label={group.nextLabel?.trim() || 'Next stage'}
              disabled={isCurrentOnly || !next || !accessible.has(next.viewId)}
              onClick={() =>
                next && onNavigateView?.(next.viewId, { replace: false })
              }
              className="w-fit justify-self-end px-2 text-muted-foreground hover:text-foreground"
            >
              {nextLabel} <ChevronRight className="ml-1 h-4 w-4" />
            </Button>
          ) : (
            <span aria-hidden="true" />
          )}
        </div>
      )}
    </nav>
  );
}
