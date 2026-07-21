import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from '@/shared/components/ui/card';
import { ArrowDown, ArrowUp, Plus, Trash2 } from 'lucide-react';
import {
  ReportDefinition,
  ReportViewDefinition,
  ReportViewGroupDefinition,
  ReportViewNavigationMode,
  ReportViewStageSource,
} from '../../types';

interface ViewsEditorV2Props {
  definition: ReportDefinition;
  onChange: (definition: ReportDefinition) => void;
}

function randomId(prefix: string): string {
  return `${prefix}_${Math.random().toString(36).slice(2, 7)}`;
}

function newView(): ReportViewDefinition {
  const id = randomId('view');
  return {
    id,
    title: 'New view',
    layout: { id: `${id}_root`, columns: 1, rows: 1, items: [] },
  };
}

function defaultStageSource(
  definition: ReportDefinition
): ReportViewStageSource {
  const filter = definition.filters[0];
  if (filter) return { type: 'filter', filterId: filter.id };
  const block = definition.blocks.find((candidate) => !candidate.lazy);
  if (block) return { type: 'block', blockId: block.id, field: 'status' };
  return { type: 'filter', filterId: '' };
}

function newGroup(
  mode: ReportViewNavigationMode,
  definition: ReportDefinition
): ReportViewGroupDefinition {
  const initialViews = (definition.views ?? []).slice(0, 2);
  if (mode === 'tabs') {
    return {
      id: randomId('tabs'),
      mode,
      viewIds: initialViews.map((view) => view.id),
      access: 'all',
    };
  }
  return {
    id: randomId('stages'),
    mode,
    stages: initialViews.map((view) => ({
      viewId: view.id,
      value: view.id,
    })),
    currentFrom: defaultStageSource(definition),
    access: 'through_current',
    showPreviousNext: true,
    followCurrentOnAdvance: true,
  };
}

function groupViewIds(group: ReportViewGroupDefinition): string[] {
  return group.mode === 'stages'
    ? (group.stages ?? []).map((stage) => stage.viewId)
    : (group.viewIds ?? []);
}

export function ViewsEditorV2({ definition, onChange }: ViewsEditorV2Props) {
  const views = definition.views ?? [];
  const groups = definition.viewGroups ?? [];

  const updateViews = (next: ReportViewDefinition[]) =>
    onChange({ ...definition, views: next });

  const updateView = (
    id: string,
    updater: (view: ReportViewDefinition) => ReportViewDefinition
  ) =>
    updateViews(views.map((view) => (view.id === id ? updater(view) : view)));

  const renameView = (oldId: string, nextId: string) => {
    onChange({
      ...definition,
      views: views.map((view) => ({
        ...view,
        id: view.id === oldId ? nextId : view.id,
        parentViewId: view.parentViewId === oldId ? nextId : view.parentViewId,
      })),
      viewGroups: groups.map((group) =>
        group.mode === 'stages'
          ? {
              ...group,
              stages: (group.stages ?? []).map((stage) => ({
                ...stage,
                viewId: stage.viewId === oldId ? nextId : stage.viewId,
              })),
            }
          : {
              ...group,
              viewIds: (group.viewIds ?? []).map((viewId) =>
                viewId === oldId ? nextId : viewId
              ),
            }
      ),
    });
  };

  const deleteView = (viewId: string) => {
    const nextGroups = groups
      .map((group) =>
        group.mode === 'stages'
          ? {
              ...group,
              stages: (group.stages ?? []).filter(
                (stage) => stage.viewId !== viewId
              ),
            }
          : {
              ...group,
              viewIds: (group.viewIds ?? []).filter(
                (candidate) => candidate !== viewId
              ),
            }
      )
      .filter((group) => groupViewIds(group).length >= 2);
    onChange({
      ...definition,
      views: views
        .filter((view) => view.id !== viewId)
        .map((view) =>
          view.parentViewId === viewId ? { ...view, parentViewId: null } : view
        ),
      viewGroups: nextGroups,
    });
  };

  const updateGroups = (next: ReportViewGroupDefinition[]) =>
    onChange({ ...definition, viewGroups: next });

  const updateGroup = (
    id: string,
    updater: (group: ReportViewGroupDefinition) => ReportViewGroupDefinition
  ) =>
    updateGroups(
      groups.map((group) => (group.id === id ? updater(group) : group))
    );

  return (
    <div className="grid gap-5">
      <div className="grid gap-3">
        {views.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            No named views. Add one to enable drill-down navigation from row or
            chart clicks.
          </p>
        ) : (
          <div className="grid gap-3">
            {views.map((view, index) => (
              <Card key={view.layout?.id ?? index}>
                <CardHeader className="flex flex-row items-center justify-between gap-2 space-y-0 py-3">
                  <CardTitle className="text-sm">
                    {view.title || view.id}
                  </CardTitle>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="h-7 w-7 text-destructive"
                    aria-label={`Delete view ${view.title || view.id}`}
                    onClick={() => deleteView(view.id)}
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </Button>
                </CardHeader>
                <CardContent className="grid gap-3 pt-0 md:grid-cols-3">
                  <div className="grid gap-1.5">
                    <Label htmlFor={`view-id-${view.id}`} className="text-xs">
                      ID
                    </Label>
                    <Input
                      id={`view-id-${view.id}`}
                      value={view.id}
                      onChange={(event) =>
                        renameView(view.id, event.target.value)
                      }
                    />
                  </div>
                  <div className="grid gap-1.5">
                    <Label
                      htmlFor={`view-title-${view.id}`}
                      className="text-xs"
                    >
                      Title
                    </Label>
                    <Input
                      id={`view-title-${view.id}`}
                      value={view.title ?? ''}
                      onChange={(event) =>
                        updateView(view.id, (candidate) => ({
                          ...candidate,
                          title: event.target.value || null,
                        }))
                      }
                    />
                  </div>
                  <div className="grid gap-1.5">
                    <Label
                      htmlFor={`view-parent-${view.id}`}
                      className="text-xs"
                    >
                      Parent view
                    </Label>
                    <select
                      id={`view-parent-${view.id}`}
                      value={view.parentViewId ?? ''}
                      onChange={(event) =>
                        updateView(view.id, (candidate) => ({
                          ...candidate,
                          parentViewId: event.target.value || null,
                        }))
                      }
                      className="h-9 rounded-md border border-input bg-background px-3 text-sm"
                    >
                      <option value="">None (top-level)</option>
                      {views
                        .filter((candidate) => candidate.id !== view.id)
                        .map((candidate) => (
                          <option key={candidate.id} value={candidate.id}>
                            {candidate.title || candidate.id}
                          </option>
                        ))}
                    </select>
                  </div>
                </CardContent>
              </Card>
            ))}
          </div>
        )}
        <div>
          <Button
            type="button"
            variant="outline"
            onClick={() => updateViews([...views, newView()])}
          >
            <Plus className="mr-1 h-3.5 w-3.5" /> Add view
          </Button>
        </div>
      </div>

      <div className="grid gap-3 border-t pt-4">
        <div>
          <h3 className="text-sm font-semibold">Navigation groups</h3>
          <p className="text-xs text-muted-foreground">
            Present sibling views as freely selectable tabs or as a state-driven
            stage sequence.
          </p>
        </div>
        {groups.map((group, index) => (
          <NavigationGroupEditor
            key={`${group.mode}-${index}`}
            definition={definition}
            group={group}
            onChange={(updater) => updateGroup(group.id, updater)}
            onDelete={() =>
              updateGroups(
                groups.filter((candidate) => candidate.id !== group.id)
              )
            }
          />
        ))}
        <div className="flex flex-wrap gap-2">
          <Button
            type="button"
            variant="outline"
            disabled={views.length < 2}
            onClick={() =>
              updateGroups([...groups, newGroup('tabs', definition)])
            }
          >
            <Plus className="mr-1 h-3.5 w-3.5" /> Add tab group
          </Button>
          <Button
            type="button"
            variant="outline"
            disabled={views.length < 2}
            onClick={() =>
              updateGroups([...groups, newGroup('stages', definition)])
            }
          >
            <Plus className="mr-1 h-3.5 w-3.5" /> Add stage group
          </Button>
          {views.length < 2 ? (
            <span className="self-center text-xs text-muted-foreground">
              Add at least two views first.
            </span>
          ) : null}
        </div>
      </div>
    </div>
  );
}

interface NavigationGroupEditorProps {
  definition: ReportDefinition;
  group: ReportViewGroupDefinition;
  onChange: (
    updater: (group: ReportViewGroupDefinition) => ReportViewGroupDefinition
  ) => void;
  onDelete: () => void;
}

function NavigationGroupEditor({
  definition,
  group,
  onChange,
  onDelete,
}: NavigationGroupEditorProps) {
  const views = definition.views ?? [];
  const memberIds = groupViewIds(group);
  const unusedView = views.find((view) => !memberIds.includes(view.id));
  const moveMember = (index: number, direction: -1 | 1) => {
    if (group.mode === 'stages') {
      const stages = [...(group.stages ?? [])];
      const nextIndex = index + direction;
      if (!stages[index] || !stages[nextIndex]) return;
      [stages[index], stages[nextIndex]] = [stages[nextIndex], stages[index]];
      onChange((current) => ({ ...current, stages }));
      return;
    }
    const viewIds = [...(group.viewIds ?? [])];
    const nextIndex = index + direction;
    if (!viewIds[index] || !viewIds[nextIndex]) return;
    [viewIds[index], viewIds[nextIndex]] = [viewIds[nextIndex], viewIds[index]];
    onChange((current) => ({ ...current, viewIds }));
  };

  const addMember = () => {
    if (!unusedView) return;
    onChange((current) =>
      current.mode === 'stages'
        ? {
            ...current,
            stages: [
              ...(current.stages ?? []),
              { viewId: unusedView.id, value: unusedView.id },
            ],
          }
        : {
            ...current,
            viewIds: [...(current.viewIds ?? []), unusedView.id],
          }
    );
  };

  return (
    <Card>
      <CardHeader className="flex flex-row items-center justify-between gap-2 space-y-0 py-3">
        <CardTitle className="text-sm">
          {group.mode === 'stages' ? 'Stage navigation' : 'Tab navigation'} ·{' '}
          {group.id}
        </CardTitle>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="h-7 w-7 text-destructive"
          aria-label={`Delete navigation group ${group.id}`}
          onClick={onDelete}
        >
          <Trash2 className="h-3.5 w-3.5" />
        </Button>
      </CardHeader>
      <CardContent className="grid gap-4 pt-0">
        <div className="grid gap-3 md:grid-cols-2">
          <div className="grid gap-1.5">
            <Label htmlFor={`group-id-${group.id}`} className="text-xs">
              Group ID
            </Label>
            <Input
              id={`group-id-${group.id}`}
              value={group.id}
              onChange={(event) =>
                onChange((current) => ({
                  ...current,
                  id: event.target.value,
                }))
              }
            />
          </div>
          <div className="grid gap-1.5">
            <Label htmlFor={`group-access-${group.id}`} className="text-xs">
              Accessible views
            </Label>
            <select
              id={`group-access-${group.id}`}
              value={group.access ?? 'all'}
              onChange={(event) =>
                onChange((current) => ({
                  ...current,
                  access: event.target.value as 'all' | 'through_current',
                }))
              }
              className="h-9 rounded-md border border-input bg-background px-3 text-sm"
            >
              <option value="all">All views</option>
              <option value="through_current">Prior + current only</option>
            </select>
          </div>
        </div>

        <div className="grid gap-2">
          <Label className="text-xs">Ordered members</Label>
          {group.mode === 'stages'
            ? (group.stages ?? []).map((stage, index) => (
                <div
                  key={`${stage.viewId}-${index}`}
                  className="grid gap-2 rounded-md border p-2 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto]"
                >
                  <select
                    aria-label={`View for stage ${index + 1}`}
                    value={stage.viewId}
                    onChange={(event) =>
                      onChange((current) => ({
                        ...current,
                        stages: (current.stages ?? []).map((candidate, at) =>
                          at === index
                            ? { ...candidate, viewId: event.target.value }
                            : candidate
                        ),
                      }))
                    }
                    className="h-9 rounded-md border border-input bg-background px-3 text-sm"
                  >
                    {views.map((view) => (
                      <option key={view.id} value={view.id}>
                        {view.title || view.id}
                      </option>
                    ))}
                  </select>
                  <Input
                    aria-label={`Persisted value for stage ${index + 1}`}
                    value={stage.value}
                    placeholder="Persisted value"
                    onChange={(event) =>
                      onChange((current) => ({
                        ...current,
                        stages: (current.stages ?? []).map((candidate, at) =>
                          at === index
                            ? { ...candidate, value: event.target.value }
                            : candidate
                        ),
                      }))
                    }
                  />
                  <MemberActions
                    index={index}
                    count={(group.stages ?? []).length}
                    onMove={moveMember}
                    onDelete={() =>
                      onChange((current) => ({
                        ...current,
                        stages: (current.stages ?? []).filter(
                          (_, at) => at !== index
                        ),
                      }))
                    }
                  />
                </div>
              ))
            : (group.viewIds ?? []).map((viewId, index) => (
                <div
                  key={`${viewId}-${index}`}
                  className="grid gap-2 rounded-md border p-2 sm:grid-cols-[minmax(0,1fr)_auto]"
                >
                  <select
                    aria-label={`Tab view ${index + 1}`}
                    value={viewId}
                    onChange={(event) =>
                      onChange((current) => ({
                        ...current,
                        viewIds: (current.viewIds ?? []).map((candidate, at) =>
                          at === index ? event.target.value : candidate
                        ),
                      }))
                    }
                    className="h-9 rounded-md border border-input bg-background px-3 text-sm"
                  >
                    {views.map((view) => (
                      <option key={view.id} value={view.id}>
                        {view.title || view.id}
                      </option>
                    ))}
                  </select>
                  <MemberActions
                    index={index}
                    count={(group.viewIds ?? []).length}
                    onMove={moveMember}
                    onDelete={() =>
                      onChange((current) => ({
                        ...current,
                        viewIds: (current.viewIds ?? []).filter(
                          (_, at) => at !== index
                        ),
                      }))
                    }
                  />
                </div>
              ))}
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="w-fit"
            disabled={!unusedView}
            onClick={addMember}
          >
            <Plus className="mr-1 h-3.5 w-3.5" /> Add member
          </Button>
        </div>

        {group.mode === 'stages' ? (
          <StageSourceEditor
            definition={definition}
            groupId={group.id}
            source={group.currentFrom ?? defaultStageSource(definition)}
            onChange={(currentFrom) =>
              onChange((current) => ({ ...current, currentFrom }))
            }
          />
        ) : null}

        <div className="flex flex-wrap gap-x-5 gap-y-2 text-sm">
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              checked={group.showPreviousNext ?? false}
              onChange={(event) =>
                onChange((current) => ({
                  ...current,
                  showPreviousNext: event.target.checked,
                }))
              }
            />
            Show Previous / Next
          </label>
          {group.mode === 'stages' ? (
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={group.followCurrentOnAdvance ?? false}
                onChange={(event) =>
                  onChange((current) => ({
                    ...current,
                    followCurrentOnAdvance: event.target.checked,
                  }))
                }
              />
              Follow current stage after save/action
            </label>
          ) : null}
        </div>
      </CardContent>
    </Card>
  );
}

interface MemberActionsProps {
  index: number;
  count: number;
  onMove: (index: number, direction: -1 | 1) => void;
  onDelete: () => void;
}

function MemberActions({ index, count, onMove, onDelete }: MemberActionsProps) {
  return (
    <div className="flex items-center justify-end gap-1">
      <Button
        type="button"
        variant="ghost"
        size="icon"
        className="h-8 w-8"
        aria-label={`Move member ${index + 1} up`}
        disabled={index === 0}
        onClick={() => onMove(index, -1)}
      >
        <ArrowUp className="h-3.5 w-3.5" />
      </Button>
      <Button
        type="button"
        variant="ghost"
        size="icon"
        className="h-8 w-8"
        aria-label={`Move member ${index + 1} down`}
        disabled={index === count - 1}
        onClick={() => onMove(index, 1)}
      >
        <ArrowDown className="h-3.5 w-3.5" />
      </Button>
      <Button
        type="button"
        variant="ghost"
        size="icon"
        className="h-8 w-8 text-destructive"
        aria-label={`Remove member ${index + 1}`}
        onClick={onDelete}
      >
        <Trash2 className="h-3.5 w-3.5" />
      </Button>
    </div>
  );
}

interface StageSourceEditorProps {
  definition: ReportDefinition;
  groupId: string;
  source: ReportViewStageSource;
  onChange: (source: ReportViewStageSource) => void;
}

function StageSourceEditor({
  definition,
  groupId,
  source,
  onChange,
}: StageSourceEditorProps) {
  const availableBlocks = definition.blocks.filter((block) => !block.lazy);
  const typeId = `stage-source-type-${groupId}`;
  const filterId = `stage-source-filter-${groupId}`;
  const blockId = `stage-source-block-${groupId}`;
  const fieldId = `stage-source-field-${groupId}`;
  return (
    <div className="grid gap-3 rounded-md border bg-muted/20 p-3 md:grid-cols-3">
      <div className="grid gap-1.5">
        <Label className="text-xs" htmlFor={typeId}>
          Current stage comes from
        </Label>
        <select
          id={typeId}
          value={source.type}
          onChange={(event) =>
            onChange(
              event.target.value === 'block'
                ? {
                    type: 'block',
                    blockId: availableBlocks[0]?.id ?? '',
                    field: 'status',
                  }
                : {
                    type: 'filter',
                    filterId: definition.filters[0]?.id ?? '',
                  }
            )
          }
          className="h-9 rounded-md border border-input bg-background px-3 text-sm"
        >
          <option value="filter">Filter value</option>
          <option value="block">Rendered block field</option>
        </select>
      </div>
      {source.type === 'filter' ? (
        <div className="grid gap-1.5 md:col-span-2">
          <Label className="text-xs" htmlFor={filterId}>
            Filter
          </Label>
          <select
            id={filterId}
            value={source.filterId}
            onChange={(event) =>
              onChange({ type: 'filter', filterId: event.target.value })
            }
            className="h-9 rounded-md border border-input bg-background px-3 text-sm"
          >
            {definition.filters.length === 0 ? (
              <option value="">No filters available</option>
            ) : null}
            {definition.filters.map((filter) => (
              <option key={filter.id} value={filter.id}>
                {filter.label || filter.id}
              </option>
            ))}
          </select>
        </div>
      ) : (
        <>
          <div className="grid gap-1.5">
            <Label className="text-xs" htmlFor={blockId}>
              Block
            </Label>
            <select
              id={blockId}
              value={source.blockId}
              onChange={(event) =>
                onChange({ ...source, blockId: event.target.value })
              }
              className="h-9 rounded-md border border-input bg-background px-3 text-sm"
            >
              {availableBlocks.length === 0 ? (
                <option value="">No blocks available</option>
              ) : null}
              {availableBlocks.map((block) => (
                <option key={block.id} value={block.id}>
                  {block.title || block.id}
                </option>
              ))}
            </select>
          </div>
          <div className="grid gap-1.5">
            <Label className="text-xs" htmlFor={fieldId}>
              Field
            </Label>
            <Input
              id={fieldId}
              value={source.field}
              onChange={(event) =>
                onChange({ ...source, field: event.target.value })
              }
              placeholder="status"
            />
          </div>
        </>
      )}
    </div>
  );
}
