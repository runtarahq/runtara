import { Plus, Trash2 } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { Checkbox } from '@/shared/components/ui/checkbox';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import {
  ReportLayoutNode,
  ReportViewBreadcrumb,
  ReportViewDefinition,
} from '../../../types';
import { extractLayoutBlockReferences, slugify } from '../../../utils';
import { WizardBlock, WizardFilter, WizardGrid } from '../wizardTypes';

const NO_PARENT = '__none__';
const ANY_BLOCK = '__none__';
const INHERIT_MAIN_LAYOUT = '__inherit__';

interface ViewsStepProps {
  views: ReportViewDefinition[];
  blocks: WizardBlock[];
  grids: WizardGrid[];
  filters: WizardFilter[];
  onChange: (next: ReportViewDefinition[]) => void;
}

export function ViewsStep({
  views,
  blocks,
  grids,
  filters,
  onChange,
}: ViewsStepProps) {
  function addView() {
    const id = uniqueViewId(views, 'view');
    onChange([
      ...views,
      {
        id,
        title: 'New view',
        layout: layoutForSelectedBlocks(
          grids,
          blocks,
          blocks.map((block) => block.id)
        ),
      },
    ]);
  }

  function updateView(index: number, patch: Partial<ReportViewDefinition>) {
    onChange(
      views.map((view, currentIndex) =>
        currentIndex === index ? { ...view, ...patch } : view
      )
    );
  }

  function renameView(index: number, nextId: string) {
    const previousId = views[index].id;
    const id = uniqueViewId(
      views.filter((_, currentIndex) => currentIndex !== index),
      slugify(nextId).replace(/-/g, '_') || 'view'
    );
    onChange(
      views.map((view, currentIndex) => {
        const nextView =
          currentIndex === index
            ? { ...view, id }
            : {
                ...view,
                parentViewId:
                  view.parentViewId === previousId ? id : view.parentViewId,
                breadcrumb: view.breadcrumb?.map((breadcrumb) => ({
                  ...breadcrumb,
                  viewId:
                    breadcrumb.viewId === previousId ? id : breadcrumb.viewId,
                })),
              };
        return nextView;
      })
    );
  }

  function removeView(index: number) {
    const removedId = views[index].id;
    onChange(
      views
        .filter((_, currentIndex) => currentIndex !== index)
        .map((view) => ({
          ...view,
          parentViewId:
            view.parentViewId === removedId ? undefined : view.parentViewId,
          breadcrumb: view.breadcrumb?.filter(
            (breadcrumb) => breadcrumb.viewId !== removedId
          ),
        }))
    );
  }

  return (
    <div className="grid gap-3">
      <div className="flex items-center justify-between">
        <span className="text-sm text-muted-foreground">
          {views.length === 0
            ? 'No named views yet.'
            : `${views.length} view${views.length === 1 ? '' : 's'} configured.`}
        </span>
        <Button type="button" variant="outline" size="sm" onClick={addView}>
          <Plus className="mr-2 h-4 w-4" />
          Add view
        </Button>
      </div>

      {views.length === 0 ? (
        <div className="rounded-md border border-dashed bg-muted/20 p-6 text-center text-sm text-muted-foreground">
          Add views to define drilldowns, breadcrumb parents, and per-view block
          layouts.
        </div>
      ) : (
        <div className="grid gap-3">
          {views.map((view, index) => (
            <ViewCard
              key={`${view.id}-${index}`}
              view={view}
              views={views}
              blocks={blocks}
              grids={grids}
              filters={filters}
              onRename={(id) => renameView(index, id)}
              onChange={(patch) => updateView(index, patch)}
              onRemove={() => removeView(index)}
            />
          ))}
        </div>
      )}
    </div>
  );
}

function ViewCard({
  view,
  views,
  blocks,
  grids,
  filters,
  onRename,
  onChange,
  onRemove,
}: {
  view: ReportViewDefinition;
  views: ReportViewDefinition[];
  blocks: WizardBlock[];
  grids: WizardGrid[];
  filters: WizardFilter[];
  onRename: (id: string) => void;
  onChange: (patch: Partial<ReportViewDefinition>) => void;
  onRemove: () => void;
}) {
  const selectedBlockIds =
    view.layout === undefined
      ? blocks.map((block) => block.id)
      : extractLayoutBlockReferences(view.layout);
  const selectedBlockSet = new Set(selectedBlockIds);
  const clearFilters = view.clearFiltersOnBack ?? [];
  const titleFromBlock = view.titleFromBlock;

  function setSelectedBlocks(nextIds: string[]) {
    onChange({ layout: layoutForSelectedBlocks(grids, blocks, nextIds) });
  }

  function toggleBlock(blockId: string, checked: boolean | 'indeterminate') {
    const next = checked
      ? [...selectedBlockIds, blockId]
      : selectedBlockIds.filter((id) => id !== blockId);
    setSelectedBlocks(Array.from(new Set(next)));
  }

  return (
    <article className="grid gap-3 rounded-md border bg-background p-3">
      <div className="grid gap-3 lg:grid-cols-[160px_minmax(0,1fr)_minmax(0,1fr)_auto]">
        <div className="grid gap-1.5">
          <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            View ID
          </Label>
          <Input
            value={view.id}
            onChange={(event) => onRename(event.target.value)}
          />
        </div>
        <div className="grid gap-1.5">
          <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Title
          </Label>
          <Input
            value={view.title ?? ''}
            placeholder="View title"
            onChange={(event) =>
              onChange({ title: event.target.value || undefined })
            }
          />
        </div>
        <div className="grid gap-1.5">
          <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Parent breadcrumb
          </Label>
          <Select
            value={view.parentViewId ?? NO_PARENT}
            onValueChange={(parentViewId) =>
              onChange({
                parentViewId:
                  parentViewId === NO_PARENT ? undefined : parentViewId,
              })
            }
          >
            <SelectTrigger>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={NO_PARENT}>No parent</SelectItem>
              {views
                .filter((candidate) => candidate.id !== view.id)
                .map((candidate) => (
                  <SelectItem key={candidate.id} value={candidate.id}>
                    {candidate.title || candidate.id}
                  </SelectItem>
                ))}
            </SelectContent>
          </Select>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          onClick={onRemove}
          aria-label={`Remove ${view.title || view.id}`}
          className="self-end"
        >
          <Trash2 className="h-4 w-4" />
        </Button>
      </div>

      <div className="grid gap-3 lg:grid-cols-3">
        <div className="grid gap-1.5">
          <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Title from filter path
          </Label>
          <Input
            value={view.titleFrom ?? ''}
            placeholder="filters.order_id"
            onChange={(event) =>
              onChange({ titleFrom: event.target.value || undefined })
            }
          />
        </div>
        <div className="grid gap-1.5">
          <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Title from block
          </Label>
          <Select
            value={titleFromBlock?.block ?? ANY_BLOCK}
            onValueChange={(blockId) =>
              onChange({
                titleFromBlock:
                  blockId === ANY_BLOCK
                    ? undefined
                    : {
                        block: blockId,
                        field: titleFromBlock?.field,
                      },
              })
            }
          >
            <SelectTrigger>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={ANY_BLOCK}>No block title</SelectItem>
              {blocks.map((block) => (
                <SelectItem key={block.id} value={block.id}>
                  {block.title || block.id}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="grid gap-1.5">
          <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Block title field
          </Label>
          <Input
            value={titleFromBlock?.field ?? ''}
            disabled={!titleFromBlock?.block}
            placeholder="name"
            onChange={(event) =>
              titleFromBlock?.block
                ? onChange({
                    titleFromBlock: {
                      block: titleFromBlock.block,
                      field: event.target.value || undefined,
                    },
                  })
                : undefined
            }
          />
        </div>
      </div>

      <div className="grid gap-3 lg:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]">
        <div className="grid gap-1.5">
          <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Clear filters on back
          </Label>
          <Input
            value={clearFilters.join(', ')}
            placeholder={filters.map((filter) => filter.id).join(', ')}
            onChange={(event) =>
              onChange({ clearFiltersOnBack: parseCsv(event.target.value) })
            }
          />
        </div>
        <div className="grid gap-1.5">
          <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Layout mode
          </Label>
          <Select
            value={view.layout === undefined ? INHERIT_MAIN_LAYOUT : 'custom'}
            onValueChange={(value) =>
              onChange({
                layout:
                  value === INHERIT_MAIN_LAYOUT
                    ? undefined
                    : layoutForSelectedBlocks(grids, blocks, selectedBlockIds),
              })
            }
          >
            <SelectTrigger>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={INHERIT_MAIN_LAYOUT}>
                Inherit main layout
              </SelectItem>
              <SelectItem value="custom">
                Choose blocks for this view
              </SelectItem>
            </SelectContent>
          </Select>
        </div>
      </div>

      {view.layout !== undefined ? (
        <div className="grid gap-2">
          <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Blocks in this view
          </Label>
          <div className="grid gap-2 sm:grid-cols-2 xl:grid-cols-3">
            {blocks.map((block) => (
              <label
                key={block.id}
                className="flex min-h-10 items-center gap-2 rounded-md border bg-muted/10 px-3 py-2 text-sm"
              >
                <Checkbox
                  checked={selectedBlockSet.has(block.id)}
                  onCheckedChange={(checked) => toggleBlock(block.id, checked)}
                />
                <span className="truncate">{block.title || block.id}</span>
              </label>
            ))}
          </div>
        </div>
      ) : null}

      <BreadcrumbEditor
        breadcrumbs={view.breadcrumb ?? []}
        views={views}
        filters={filters}
        onChange={(breadcrumb) => onChange({ breadcrumb })}
      />
    </article>
  );
}

function BreadcrumbEditor({
  breadcrumbs,
  views,
  filters,
  onChange,
}: {
  breadcrumbs: ReportViewBreadcrumb[];
  views: ReportViewDefinition[];
  filters: WizardFilter[];
  onChange: (next: ReportViewBreadcrumb[]) => void;
}) {
  return (
    <div className="grid gap-2 rounded-md border bg-muted/10 p-3">
      <div className="flex items-center justify-between gap-2">
        <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
          Manual breadcrumbs
        </Label>
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="h-7"
          onClick={() =>
            onChange([
              ...breadcrumbs,
              {
                label: 'Back',
                viewId: views[0]?.id,
                clearFilters: [],
              },
            ])
          }
        >
          <Plus className="mr-1 h-3 w-3" />
          Add breadcrumb
        </Button>
      </div>
      {breadcrumbs.length === 0 ? (
        <p className="text-xs text-muted-foreground">
          Parent views generate breadcrumbs automatically. Add manual entries
          only when the trail needs custom labels or filter clearing.
        </p>
      ) : (
        <div className="grid gap-2">
          {breadcrumbs.map((breadcrumb, index) => (
            <div
              key={`${breadcrumb.label}-${index}`}
              className="grid gap-2 rounded-md border bg-background p-2 lg:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_minmax(0,1fr)_auto]"
            >
              <Input
                value={breadcrumb.label}
                placeholder="Label"
                onChange={(event) =>
                  onChange(
                    breadcrumbs.map((item, currentIndex) =>
                      currentIndex === index
                        ? { ...item, label: event.target.value }
                        : item
                    )
                  )
                }
              />
              <Select
                value={breadcrumb.viewId ?? NO_PARENT}
                onValueChange={(viewId) =>
                  onChange(
                    breadcrumbs.map((item, currentIndex) =>
                      currentIndex === index
                        ? {
                            ...item,
                            viewId: viewId === NO_PARENT ? undefined : viewId,
                          }
                        : item
                    )
                  )
                }
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value={NO_PARENT}>Label only</SelectItem>
                  {views.map((view) => (
                    <SelectItem key={view.id} value={view.id}>
                      {view.title || view.id}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <Input
                value={(breadcrumb.clearFilters ?? []).join(', ')}
                placeholder={filters.map((filter) => filter.id).join(', ')}
                onChange={(event) =>
                  onChange(
                    breadcrumbs.map((item, currentIndex) =>
                      currentIndex === index
                        ? {
                            ...item,
                            clearFilters: parseCsv(event.target.value),
                          }
                        : item
                    )
                  )
                }
              />
              <Button
                type="button"
                size="icon"
                variant="ghost"
                onClick={() =>
                  onChange(
                    breadcrumbs.filter(
                      (_, currentIndex) => currentIndex !== index
                    )
                  )
                }
                aria-label="Remove breadcrumb"
              >
                <Trash2 className="h-4 w-4" />
              </Button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function layoutForSelectedBlocks(
  grids: WizardGrid[],
  blocks: WizardBlock[],
  blockIds: string[]
): ReportLayoutNode[] {
  const selected = new Set(blockIds);
  const layout: ReportLayoutNode[] = [];

  for (const grid of grids) {
    const gridBlocks = blocks
      .filter(
        (block) => block.placement.gridId === grid.id && selected.has(block.id)
      )
      .sort(
        (a, b) =>
          a.placement.row - b.placement.row ||
          a.placement.column - b.placement.column
      );
    if (gridBlocks.length === 0) continue;

    const gridNode: Extract<ReportLayoutNode, { type: 'grid' }> = {
      id: `${grid.id}_view_grid`,
      type: 'grid',
      columns: grid.columns,
      items: gridBlocks.map((block) => ({
        id: `node_${block.id}`,
        blockId: block.id,
        colSpan: 1,
        rowSpan: 1,
      })),
    };

    if (grid.title || grid.description) {
      layout.push({
        id: `${grid.id}_view_section`,
        type: 'section',
        title: grid.title,
        description: grid.description,
        children: [gridNode],
      });
    } else {
      layout.push({ ...gridNode, id: `${grid.id}_view` });
    }
  }

  return layout;
}

function parseCsv(value: string): string[] {
  return value
    .split(',')
    .map((part) => part.trim())
    .filter(Boolean);
}

function uniqueViewId(views: ReportViewDefinition[], base: string): string {
  const stem = slugify(base).replace(/-/g, '_') || 'view';
  const used = new Set(views.map((view) => view.id));
  if (!used.has(stem)) return stem;
  let index = 2;
  while (used.has(`${stem}_${index}`)) index += 1;
  return `${stem}_${index}`;
}
