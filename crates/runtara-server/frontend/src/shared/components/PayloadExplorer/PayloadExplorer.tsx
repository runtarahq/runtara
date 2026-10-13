import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from 'react';
import {
  ChevronDown,
  ChevronUp,
  ChevronsDownUp,
  ChevronsUpDown,
  Copy,
  ListFilter,
  Search,
  X,
} from 'lucide-react';
import { toast } from 'sonner';
import { cn } from '@/lib/utils';
import { Button } from '@/shared/components/ui/button';
import {
  hasElidedPayload,
  isTruncatedPayload,
} from '@/shared/utils/truncated-payload';
import {
  EXPAND_ALL_MAX_BRANCHES,
  SEARCH_MAX_NODES,
  ancestorIds,
  childCount,
  collectBranchIds,
  flattenRows,
  formatCount,
  formatPath,
  formatSize,
  getAt,
  groupValue,
  initialExpanded,
  isBranch,
  kindOf,
  matchFilter,
  measurePayload,
  plural,
  searchPayload,
  type PathSegment,
  type PayloadSize,
  type SearchResult,
  type TreeRow,
} from './model';
import { PayloadTree, type FieldLabel } from './PayloadTree';
import { PayloadRaw } from './PayloadRaw';
import { PayloadInspector } from './PayloadInspector';

export interface PayloadSection {
  id: string;
  label: string;
  /** The parsed JSON value. `null` or `undefined` shows the empty state. */
  value: unknown;
  emptyState?: { title: string; description?: string };
  /** Extra context shown above the tree, e.g. "Updated 3 hr ago". */
  meta?: ReactNode;
  /** Friendly labels for top-level keys, e.g. from a state schema. */
  fieldLabels?: Record<string, FieldLabel>;
}

export interface PayloadExplorerProps {
  sections: PayloadSection[];
  defaultSectionId?: string;
  /**
   * Fetches a section's full value when it contains server-elided
   * placeholders. Without it, placeholders show their size only.
   */
  loadFullValue?: (sectionId: string) => Promise<unknown>;
  /** Height of the scrolling area in pixels (420 by default, 240 compact). */
  maxHeight?: number;
  /**
   * Fits tight spaces (a step's details, a side panel): one toolbar row with
   * icon actions, and a path-only strip that appears once a field is selected.
   */
  compact?: boolean;
  ariaLabel?: string;
  className?: string;
}

interface SectionView {
  expanded: ReadonlySet<string>;
  selectedId: string | null;
  matchIndex: number;
}

const SEARCH_DEBOUNCE_MS = 150;

// Payloads are immutable once fetched, so search results and sizes are cached
// per value: re-renders (polling, typing elsewhere) never re-walk a payload.
const searchCache = new WeakMap<object, Map<string, SearchResult>>();
const sizeCache = new WeakMap<object, PayloadSize>();

function cachedSearch(value: unknown, query: string): SearchResult {
  if (typeof value !== 'object' || value === null)
    return searchPayload(value, query);
  let byQuery = searchCache.get(value);
  if (!byQuery) {
    byQuery = new Map();
    searchCache.set(value, byQuery);
  }
  let result = byQuery.get(query);
  if (!result) {
    result = searchPayload(value, query);
    // Keep the last few queries only; each result can hold 1,000 paths.
    if (byQuery.size >= 8)
      byQuery.delete(byQuery.keys().next().value as string);
    byQuery.set(query, result);
  }
  return result;
}

function cachedSize(value: unknown): PayloadSize {
  if (typeof value !== 'object' || value === null) return measurePayload(value);
  let size = sizeCache.get(value);
  if (!size) {
    size = measurePayload(value);
    sizeCache.set(value, size);
  }
  return size;
}

function isEmptyValue(value: unknown) {
  return value === null || value === undefined;
}

async function writeClipboard(text: string, message: string) {
  try {
    await navigator.clipboard.writeText(text);
    toast.success(message);
  } catch {
    toast.error('Could not copy: the browser blocked clipboard access.');
  }
}

/**
 * Explores one or more JSON payloads (a run's input, output and state) as a
 * searchable, virtualized tree, with a raw JSON view, path copying and
 * graceful handling of very large or server-elided values.
 */
export function PayloadExplorer({
  sections,
  defaultSectionId,
  loadFullValue,
  maxHeight,
  compact = false,
  ariaLabel = 'Payload',
  className,
}: PayloadExplorerProps) {
  const idPrefix = `payload-${useId().replace(/[^a-zA-Z0-9_-]/g, '')}`;
  const searchRef = useRef<HTMLInputElement>(null);
  const treeWrapRef = useRef<HTMLDivElement>(null);

  const [activeId, setActiveId] = useState(
    () =>
      sections.find((s) => s.id === defaultSectionId)?.id ??
      sections[0]?.id ??
      ''
  );
  const active = sections.find((s) => s.id === activeId) ?? sections[0];

  const [fullValues, setFullValues] = useState<Record<string, unknown>>({});
  const [loadingFull, setLoadingFull] = useState<string | null>(null);
  const valueOf = useCallback(
    (section: PayloadSection) =>
      section.id in fullValues ? fullValues[section.id] : section.value,
    [fullValues]
  );

  // Per-section folding, selection and match position. A section's first view
  // is created once and kept stable so memoized rows are not rebuilt.
  const defaultViews = useRef<Record<string, SectionView>>({});
  const [views, setViews] = useState<Record<string, SectionView>>({});
  const viewOf = (section: PayloadSection): SectionView => {
    const existing = views[section.id];
    if (existing) return existing;
    defaultViews.current[section.id] ??= {
      expanded: initialExpanded(valueOf(section)),
      selectedId: null,
      matchIndex: 0,
    };
    return defaultViews.current[section.id];
  };
  const updateView = (
    sectionId: string,
    update: (view: SectionView) => SectionView
  ) => {
    setViews((prev) => {
      const section = sections.find((s) => s.id === sectionId);
      const base = prev[sectionId] ?? (section ? viewOf(section) : null);
      return base ? { ...prev, [sectionId]: update(base) } : prev;
    });
  };

  const [input, setInput] = useState('');
  const [query, setQuery] = useState('');
  useEffect(() => {
    const timer = setTimeout(() => setQuery(input.trim()), SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [input]);
  const [onlyMatches, setOnlyMatches] = useState(false);
  const [mode, setMode] = useState<'tree' | 'json'>('tree');

  const searches: Record<string, SearchResult> = {};
  for (const section of sections)
    searches[section.id] = cachedSearch(valueOf(section), query);

  // A new query opens the branches holding its matches in every section,
  // selects the first match, and moves to a section that has any.
  const revealedQuery = useRef('');
  useEffect(() => {
    if (query === revealedQuery.current) return;
    revealedQuery.current = query;
    if (!query) return;
    setViews((prev) => {
      const next = { ...prev };
      for (const section of sections) {
        const { matches } = cachedSearch(valueOf(section), query);
        if (!matches.length) continue;
        const base = prev[section.id] ?? viewOf(section);
        const expanded = new Set(base.expanded);
        const value = valueOf(section);
        for (const match of matches) {
          for (const id of ancestorIds(value, match.segments)) expanded.add(id);
        }
        next[section.id] = {
          expanded,
          matchIndex: 0,
          selectedId: matches[0].path,
        };
      }
      return next;
    });
    if (active && !cachedSearch(valueOf(active), query).matches.length) {
      const withMatches = sections.find(
        (s) => cachedSearch(valueOf(s), query).matches.length
      );
      if (withMatches) setActiveId(withMatches.id);
    }
    // Runs once per query; the ref guards against re-running on other changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [query]);

  const value = active ? valueOf(active) : undefined;
  const view = active ? viewOf(active) : null;
  const search = active ? searches[active.id] : null;
  const expanded = view?.expanded;
  const filter = useMemo(
    () =>
      onlyMatches && search?.query ? matchFilter(value, search.matches) : null,
    [onlyMatches, search, value]
  );
  const rows = useMemo(
    () => (expanded ? flattenRows(value, expanded, filter) : []),
    [value, expanded, filter]
  );
  const matchedPaths = useMemo(
    () => new Set(search?.matches.map((m) => m.path) ?? []),
    [search]
  );
  const selectedRow = useMemo(
    () =>
      view?.selectedId == null
        ? null
        : (rows.find((r) => r.id === view.selectedId) ?? null),
    [rows, view?.selectedId]
  );
  const size = active && !isEmptyValue(value) ? cachedSize(value) : null;
  const matchCount = search?.matches.length ?? 0;
  const currentMatch =
    view && matchCount ? search!.matches[view.matchIndex % matchCount] : null;

  // ---- Actions -------------------------------------------------------------

  const ensureFull = async (section: PayloadSection): Promise<unknown> => {
    if (section.id in fullValues) return fullValues[section.id];
    if (!loadFullValue) return undefined;
    setLoadingFull(section.id);
    try {
      const full = await loadFullValue(section.id);
      setFullValues((prev) => ({ ...prev, [section.id]: full }));
      return full;
    } catch (error) {
      toast.error(
        `Could not load the full ${section.label.toLowerCase()}: ${
          error instanceof Error ? error.message : 'request failed'
        }`
      );
      return undefined;
    } finally {
      setLoadingFull(null);
    }
  };

  const loadFull = async () => {
    if (!active) return;
    const full = await ensureFull(active);
    if (full !== undefined)
      toast.success(`Loaded the full ${active.label.toLowerCase()}`);
  };

  const rowValue = (row: TreeRow, root: unknown) => {
    const at = getAt(root, row.segments);
    return row.type === 'group' && row.range ? groupValue(at, row.range) : at;
  };

  const copyValue = async (row: TreeRow) => {
    if (!active) return;
    let copied = rowValue(row, value);
    if (hasElidedPayload(copied) && loadFullValue) {
      const full = await ensureFull(active);
      if (full !== undefined) copied = rowValue(row, full);
    }
    if (kindOf(copied) === 'truncated' && isTruncatedPayload(copied)) {
      // Only the stored beginning exists; say so rather than imply it is whole.
      await writeClipboard(
        copied._preview,
        'Copied the stored part of this value'
      );
      return;
    }
    await writeClipboard(
      typeof copied === 'string' ? copied : JSON.stringify(copied, null, 2),
      'Copied value'
    );
  };

  const copyPath = (row: TreeRow) => {
    void writeClipboard(row.path, `Copied ${row.path}`);
  };

  const copySection = async () => {
    if (!active) return;
    let copied = value;
    if (hasElidedPayload(copied) && loadFullValue) {
      const full = await ensureFull(active);
      if (full !== undefined) copied = full;
    }
    await writeClipboard(
      typeof copied === 'string' ? copied : JSON.stringify(copied, null, 2),
      `Copied ${active.label.toLowerCase()} JSON`
    );
  };

  const select = (id: string) => {
    if (active) updateView(active.id, (v) => ({ ...v, selectedId: id }));
  };

  const toggle = (row: TreeRow) => {
    if (!active) return;
    updateView(active.id, (v) => {
      const next = new Set(v.expanded);
      if (next.has(row.id)) next.delete(row.id);
      else next.add(row.id);
      return { ...v, expanded: next };
    });
  };

  const reveal = (segments: readonly PathSegment[]) => {
    if (!active) return;
    const path = formatPath(segments);
    updateView(active.id, (v) => {
      const next = new Set(v.expanded);
      for (const id of ancestorIds(value, segments)) next.add(id);
      return {
        ...v,
        expanded: next,
        selectedId: segments.length ? path : null,
      };
    });
  };

  const goToMatch = (delta: number) => {
    if (!active || !matchCount || !view) return;
    const index = (view.matchIndex + delta + matchCount) % matchCount;
    const match = search!.matches[index];
    updateView(active.id, (v) => {
      const next = new Set(v.expanded);
      for (const id of ancestorIds(value, match.segments)) next.add(id);
      return { expanded: next, matchIndex: index, selectedId: match.path };
    });
  };

  const expandAll = () => {
    if (!active) return;
    const { ids, truncated } = collectBranchIds(value);
    updateView(active.id, (v) => ({ ...v, expanded: new Set(ids) }));
    if (truncated) {
      toast.info(
        `Opened the first ${formatCount(EXPAND_ALL_MAX_BRANCHES)} branches. Search to reach the rest.`
      );
    }
  };

  const collapseAll = () => {
    if (active) updateView(active.id, (v) => ({ ...v, expanded: new Set() }));
  };

  const switchSection = (id: string) => {
    setActiveId(id);
  };

  const onTabKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
    event.preventDefault();
    const index = sections.findIndex((s) => s.id === active?.id);
    const next =
      sections[
        (index + (event.key === 'ArrowRight' ? 1 : -1) + sections.length) %
          sections.length
      ];
    switchSection(next.id);
    document.getElementById(`${idPrefix}-tab-${next.id}`)?.focus();
  };

  const onSearchKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'Enter') {
      event.preventDefault();
      const typed = input.trim();
      if (typed !== query) setQuery(typed);
      else goToMatch(event.shiftKey ? -1 : 1);
    } else if (event.key === 'Escape' && input) {
      event.preventDefault();
      setInput('');
      setQuery('');
    } else if (event.key === 'ArrowDown') {
      event.preventDefault();
      treeWrapRef.current?.querySelector<HTMLElement>('[role="tree"]')?.focus();
    }
  };

  const onRootKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    const target = event.target as HTMLElement;
    if (
      event.key === '/' &&
      !event.metaKey &&
      !event.ctrlKey &&
      !/^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName) &&
      !target.isContentEditable
    ) {
      event.preventDefault();
      searchRef.current?.focus();
      searchRef.current?.select();
    }
  };

  if (!active || !view || !search) return null;

  const empty = isEmptyValue(value);
  const height = maxHeight ?? (compact ? 240 : 420);
  const canLoadFull = Boolean(loadFullValue) && !(active.id in fullValues);

  const sizeLabel = size && (
    <span
      className="tabular-nums text-muted-foreground"
      title="Approximate JSON size"
    >
      {size.complete ? '~' : '>'}
      {formatSize(size.bytes + size.elidedBytes + size.truncatedBytes)}
    </span>
  );

  const sectionPicker =
    sections.length > 1 ? (
      <div
        role="tablist"
        aria-label={`${ariaLabel} sections`}
        className={cn(
          'flex flex-wrap gap-0.5 rounded-lg border bg-muted/40 p-0.5',
          compact && 'rounded-md'
        )}
      >
        {sections.map((section) => {
          const selected = section.id === active.id;
          const result = searches[section.id];
          const sectionValue = valueOf(section);
          const count = query
            ? `${formatCount(result.matches.length)}${result.stoppedAt === 'matches' ? '+' : ''}`
            : isEmptyValue(sectionValue)
              ? '—'
              : isBranch(sectionValue)
                ? formatCount(childCount(sectionValue))
                : '1';
          return (
            <button
              key={section.id}
              id={`${idPrefix}-tab-${section.id}`}
              type="button"
              role="tab"
              aria-selected={selected}
              aria-controls={`${idPrefix}-panel`}
              tabIndex={selected ? 0 : -1}
              onClick={() => switchSection(section.id)}
              onKeyDown={onTabKeyDown}
              className={cn(
                'inline-flex items-center gap-1.5 font-medium transition-colors',
                compact
                  ? 'rounded px-2 py-0.5 text-xs'
                  : 'rounded-md px-3 py-1 text-sm',
                selected
                  ? 'bg-card text-foreground shadow-sm ring-1 ring-border'
                  : 'text-muted-foreground hover:text-foreground'
              )}
            >
              {section.label}
              <span
                className={cn(
                  'text-[11px] tabular-nums',
                  query && result.matches.length
                    ? 'rounded-full bg-warning/30 px-1.5 font-semibold text-foreground'
                    : 'text-muted-foreground'
                )}
                aria-label={
                  query ? `${result.matches.length} matches` : undefined
                }
              >
                {count}
              </span>
            </button>
          );
        })}
      </div>
    ) : (
      <span
        id={`${idPrefix}-tab-${active.id}`}
        className={cn('font-medium', compact ? 'text-xs' : 'text-sm')}
      >
        {active.label}
      </span>
    );

  const searchBox = (
    <label
      className={cn(
        'flex min-w-0 items-center gap-2 rounded-md border bg-background focus-within:border-primary focus-within:ring-2 focus-within:ring-primary/20',
        compact ? 'h-7 flex-[1_1_8rem] px-2' : 'h-8 flex-[1_1_16rem] px-2.5'
      )}
    >
      <Search
        className={cn(
          'shrink-0 text-muted-foreground',
          compact ? 'size-3.5' : 'size-4'
        )}
      />
      <input
        ref={searchRef}
        type="search"
        value={input}
        onChange={(event) => setInput(event.target.value)}
        onKeyDown={onSearchKeyDown}
        placeholder={
          compact
            ? 'Search'
            : `Search keys and values${sections.length > 1 ? ` in ${sections.map((s) => s.label.toLowerCase()).join(', ')}` : ''}`
        }
        aria-label="Search keys and values"
        spellCheck={false}
        autoComplete="off"
        className={cn(
          'min-w-0 flex-1 bg-transparent outline-none placeholder:text-muted-foreground [&::-webkit-search-cancel-button]:hidden',
          compact ? 'text-xs' : 'text-sm'
        )}
      />
      {input ? (
        <button
          type="button"
          aria-label="Clear search"
          className="grid size-5 place-items-center rounded text-muted-foreground hover:text-foreground"
          onClick={() => {
            setInput('');
            setQuery('');
            searchRef.current?.focus();
          }}
        >
          <X className="size-3.5" />
        </button>
      ) : (
        !compact && (
          <kbd className="rounded border border-b-2 px-1.5 font-mono text-[10px] text-muted-foreground">
            /
          </kbd>
        )
      )}
    </label>
  );

  const matchNav = query && (
    <div
      className="flex items-center gap-0.5 text-xs tabular-nums text-muted-foreground"
      aria-live="polite"
    >
      <span className="px-1">
        {matchCount
          ? `${formatCount((view.matchIndex % matchCount) + 1)} of ${formatCount(matchCount)}${search.stoppedAt === 'matches' ? '+' : ''}`
          : 'No matches'}
      </span>
      <IconButton
        label="Previous match"
        disabled={matchCount < 2}
        onClick={() => goToMatch(-1)}
      >
        <ChevronUp className="size-3.5" />
      </IconButton>
      <IconButton
        label="Next match"
        disabled={matchCount < 2}
        onClick={() => goToMatch(1)}
      >
        <ChevronDown className="size-3.5" />
      </IconButton>
    </div>
  );

  const modeSwitch = (
    <div
      role="group"
      aria-label="View"
      className="inline-flex overflow-hidden rounded-md border"
    >
      {(['tree', 'json'] as const).map((m) => (
        <button
          key={m}
          type="button"
          aria-pressed={mode === m}
          onClick={() => setMode(m)}
          className={cn(
            'font-medium',
            compact ? 'h-6 px-2 text-[11px]' : 'h-7 px-2.5 text-xs',
            m === 'json' && 'border-l',
            mode === m
              ? 'bg-primary/10 text-primary-text'
              : 'bg-background text-muted-foreground hover:text-foreground'
          )}
        >
          {m === 'tree' ? 'Tree' : 'JSON'}
        </button>
      ))}
    </div>
  );

  const treeDisabled = mode !== 'tree' || empty;

  return (
    <section
      aria-label={ariaLabel}
      onKeyDown={onRootKeyDown}
      className={cn(
        'min-w-0 overflow-hidden rounded-lg border bg-card text-card-foreground',
        compact && 'rounded-md',
        className
      )}
    >
      {compact ? (
        <div className="flex flex-wrap items-center gap-1.5 border-b bg-muted/30 px-2 py-1.5">
          {sectionPicker}
          {searchBox}
          {matchNav}
          <div className="ml-auto flex items-center gap-0.5">
            <IconButton
              label="Only matches"
              pressed={onlyMatches}
              onClick={() => setOnlyMatches((on) => !on)}
            >
              <ListFilter className="size-3.5" />
            </IconButton>
            <IconButton
              label="Expand all"
              disabled={treeDisabled}
              onClick={expandAll}
            >
              <ChevronsUpDown className="size-3.5" />
            </IconButton>
            <IconButton
              label="Collapse all"
              disabled={treeDisabled}
              onClick={collapseAll}
            >
              <ChevronsDownUp className="size-3.5" />
            </IconButton>
            <IconButton
              label="Copy JSON"
              disabled={empty}
              onClick={() => void copySection()}
            >
              <Copy className="size-3.5" />
            </IconButton>
            <span className="ml-1">{modeSwitch}</span>
          </div>
        </div>
      ) : (
        <>
          {/* Sections and size */}
          <div className="flex flex-wrap items-center justify-between gap-3 border-b px-4 py-2.5">
            {sectionPicker}
            <div className="flex items-center gap-3 text-xs">
              {sizeLabel}
              <Button
                variant="secondary"
                bordered
                size="sm"
                onClick={() => void copySection()}
                disabled={empty}
              >
                <Copy className="size-3.5" />
                Copy JSON
              </Button>
            </div>
          </div>

          {/* Toolbar */}
          <div className="flex flex-wrap items-center gap-x-3 gap-y-2 border-b bg-muted/30 px-4 py-2">
            {searchBox}
            {matchNav}
            <div className="flex flex-wrap items-center gap-2">
              <label className="flex cursor-pointer select-none items-center gap-1.5 text-xs text-muted-foreground">
                <input
                  type="checkbox"
                  checked={onlyMatches}
                  onChange={(event) => setOnlyMatches(event.target.checked)}
                  className="accent-primary"
                />
                Only matches
              </label>
              <Button
                variant="secondary"
                size="sm"
                onClick={expandAll}
                disabled={treeDisabled}
              >
                Expand all
              </Button>
              <Button
                variant="secondary"
                size="sm"
                onClick={collapseAll}
                disabled={treeDisabled}
              >
                Collapse all
              </Button>
              {modeSwitch}
            </div>
          </div>
        </>
      )}

      {/* Context */}
      <SectionNotes
        compact={compact}
        meta={active.meta}
        size={size}
        search={search}
        canLoadFull={canLoadFull}
        loadingFull={loadingFull === active.id}
        onLoadFull={() => void loadFull()}
      />

      {/* Content */}
      <div
        id={`${idPrefix}-panel`}
        role="tabpanel"
        aria-labelledby={`${idPrefix}-tab-${active.id}`}
        ref={treeWrapRef}
      >
        {empty ? (
          <EmptyState
            compact={compact}
            title={
              active.emptyState?.title ?? `No ${active.label.toLowerCase()}`
            }
            description={active.emptyState?.description}
          />
        ) : mode === 'json' ? (
          <PayloadRaw
            value={value}
            query={query}
            maxHeight={height}
            label={active.label}
          />
        ) : rows.length === 0 ? (
          query && onlyMatches ? (
            <EmptyState
              compact={compact}
              title={`No matches in ${active.label.toLowerCase()}`}
              description={otherSectionsHint(sections, searches, active.id)}
            />
          ) : (
            <EmptyState
              compact={compact}
              title={Array.isArray(value) ? 'Empty array' : 'Empty object'}
            />
          )
        ) : (
          <PayloadTree
            rows={rows}
            idPrefix={`${idPrefix}-row`}
            ariaLabel={`${active.label} fields`}
            maxHeight={height}
            selectedId={view.selectedId}
            query={query}
            matchedPaths={matchedPaths}
            currentMatchPath={currentMatch?.path ?? null}
            fieldLabels={active.fieldLabels}
            canLoadFull={canLoadFull}
            loadingFull={loadingFull === active.id}
            onSelect={select}
            onToggle={toggle}
            onCopyPath={copyPath}
            onCopyValue={(row) => void copyValue(row)}
            onLoadFull={() => void loadFull()}
          />
        )}
      </div>

      {/* Inspector */}
      {!empty && mode === 'tree' && (!compact || selectedRow) && (
        <div
          role="group"
          aria-label="Selected field"
          className={cn('border-t', compact ? 'px-2 py-1.5' : 'px-4 py-2.5')}
        >
          <PayloadInspector
            compact={compact}
            row={selectedRow}
            sectionLabel={active.label}
            onSelectPath={reveal}
            onCopyPath={copyPath}
            onCopyValue={(row) => void copyValue(row)}
          />
        </div>
      )}
    </section>
  );
}

function SectionNotes({
  compact,
  meta,
  size,
  search,
  canLoadFull,
  loadingFull,
  onLoadFull,
}: {
  compact: boolean;
  meta?: ReactNode;
  size: PayloadSize | null;
  search: SearchResult;
  canLoadFull: boolean;
  loadingFull: boolean;
  onLoadFull: () => void;
}) {
  const notes: ReactNode[] = [];
  if (meta) notes.push(<span key="meta">{meta}</span>);
  if (size?.elidedCount) {
    notes.push(
      <span key="elided" className="text-warning">
        {plural(size.elidedCount, 'large value')} not loaded (
        {formatSize(size.elidedBytes)})
        {canLoadFull && (
          <button
            type="button"
            onClick={onLoadFull}
            disabled={loadingFull}
            className="ml-1.5 font-medium text-primary-text hover:underline disabled:opacity-60"
          >
            {loadingFull ? 'Loading…' : 'Load full value'}
          </button>
        )}
      </span>
    );
  }
  if (size?.truncatedCount) {
    notes.push(
      <span key="truncated" className="text-warning">
        {plural(size.truncatedCount, 'value')} cut off by the server (
        {formatSize(size.truncatedBytes)} originally); only the stored part is
        shown.
      </span>
    );
  }
  if (search.stoppedAt === 'matches') {
    notes.push(
      <span key="cap">
        Showing the first {formatCount(search.matches.length)} matches. Refine
        the search to see others.
      </span>
    );
  } else if (search.stoppedAt === 'fields') {
    notes.push(
      <span key="cap">
        Searched the first {formatCount(SEARCH_MAX_NODES)} fields; matches
        further in are not listed.
      </span>
    );
  }
  if (!notes.length) return null;
  return (
    <div
      className={cn(
        'flex flex-wrap gap-x-4 gap-y-1 text-muted-foreground',
        compact ? 'px-2 pt-1.5 text-[11px]' : 'px-4 pt-2 text-xs'
      )}
      aria-live="polite"
    >
      {notes}
    </div>
  );
}

function otherSectionsHint(
  sections: PayloadSection[],
  searches: Record<string, SearchResult>,
  activeId: string
) {
  const elsewhere = sections
    .filter((s) => s.id !== activeId && searches[s.id].matches.length)
    .map(
      (s) =>
        `${formatCount(searches[s.id].matches.length)} in ${s.label.toLowerCase()}`
    );
  return elsewhere.length
    ? `Found ${elsewhere.join(', ')}.`
    : 'Nothing here matches the search.';
}

function EmptyState({
  title,
  description,
  compact = false,
}: {
  title: string;
  description?: string;
  compact?: boolean;
}) {
  return (
    <div
      className={cn(
        'flex flex-col items-center gap-1 text-center',
        compact ? 'px-3 py-4' : 'px-6 py-10'
      )}
    >
      <p
        className={cn(
          'font-medium text-muted-foreground',
          compact ? 'text-xs' : 'text-sm'
        )}
      >
        {title}
      </p>
      {description && (
        <p className="text-xs text-muted-foreground">{description}</p>
      )}
    </div>
  );
}

function IconButton({
  label,
  disabled,
  pressed,
  onClick,
  children,
}: {
  label: string;
  disabled?: boolean;
  /** Set for toggles; renders the pressed state. */
  pressed?: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      aria-pressed={pressed}
      title={label}
      disabled={disabled}
      onClick={onClick}
      className={cn(
        'grid size-6 place-items-center rounded hover:bg-primary/10 hover:text-primary-text disabled:pointer-events-none disabled:opacity-40',
        pressed ? 'bg-primary/10 text-primary-text' : 'text-muted-foreground'
      )}
    >
      {children}
    </button>
  );
}
