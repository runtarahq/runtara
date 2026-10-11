/**
 * Pure data model behind the PayloadExplorer: turning a JSON value into the
 * rows the tree shows, searching it, and sizing it.
 *
 * Everything here is written for payloads well past 1 MB:
 * - only expanded branches are flattened, so the row count follows what the
 *   reader opened, not the payload size;
 * - branches with more than CHUNK_SIZE children are split into nested groups
 *   (like browser devtools), so no single expand produces 100k rows;
 * - search, size estimation and expand-all walk with explicit stacks and stop
 *   at fixed budgets, reporting that they stopped instead of freezing the tab;
 * - the raw JSON view refuses payloads over RAW_VIEW_MAX_BYTES.
 */
import {
  isElidedStub,
  isTruncatedPayload,
} from '@/shared/utils/truncated-payload';

/** Children per group when a branch is too wide to list flat. */
export const CHUNK_SIZE = 100;
/** Characters of a string shown in a tree row; the rest is elided. */
export const ROW_TEXT_LIMIT = 300;
/** Characters of a string shown in the inspector preview. */
export const INSPECTOR_TEXT_LIMIT = 10_000;
/** Search stops collecting after this many matches. */
export const SEARCH_MAX_MATCHES = 1_000;
/** Search stops walking after visiting this many fields. */
export const SEARCH_MAX_NODES = 250_000;
/** Only the first this-many characters of each string are searched. */
export const SEARCH_STRING_SCAN_LIMIT = 100_000;
/** Above this estimated size the raw JSON view is not rendered. */
export const RAW_VIEW_MAX_BYTES = 2 * 1024 * 1024;
/** Expand-all opens at most this many branches and groups. */
export const EXPAND_ALL_MAX_BRANCHES = 2_000;
/** Size estimation stops after visiting this many fields. */
export const MEASURE_MAX_NODES = 1_000_000;

export type JsonKind =
  | 'object'
  | 'array'
  | 'string'
  | 'number'
  | 'boolean'
  | 'null'
  | 'elided'
  | 'truncated'
  | 'unknown';

export type PathSegment = string | number;

type Branch = Record<string, unknown> | unknown[];

export function kindOf(value: unknown): JsonKind {
  if (value === null) return 'null';
  if (Array.isArray(value)) return 'array';
  if (isElidedStub(value)) return 'elided';
  if (isTruncatedPayload(value)) return 'truncated';
  switch (typeof value) {
    case 'object':
      return 'object';
    case 'string':
      return 'string';
    case 'number':
      return 'number';
    case 'boolean':
      return 'boolean';
    default:
      return 'unknown';
  }
}

export function isBranch(value: unknown): value is Branch {
  const kind = kindOf(value);
  return kind === 'object' || kind === 'array';
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

const IDENTIFIER = /^[A-Za-z_$][\w$]*$/;

/**
 * Appends one segment to a path, producing the reference a reader would
 * paste: `items[2].sku`, or `headers["content-type"]` for keys that are not
 * identifiers.
 */
export function appendPath(parent: string, segment: PathSegment): string {
  if (typeof segment === 'number') return `${parent}[${segment}]`;
  if (IDENTIFIER.test(segment))
    return parent ? `${parent}.${segment}` : segment;
  return `${parent}[${JSON.stringify(segment)}]`;
}

export function formatPath(segments: readonly PathSegment[]): string {
  return segments.reduce<string>(appendPath, '');
}

export function getAt(
  root: unknown,
  segments: readonly PathSegment[]
): unknown {
  let current = root;
  for (const segment of segments) {
    if (!isBranch(current)) return undefined;
    current = (current as Record<string | number, unknown>)[segment];
  }
  return current;
}

// ---------------------------------------------------------------------------
// Child access (cached so wide objects are not re-keyed on every render)
// ---------------------------------------------------------------------------

const keyCache = new WeakMap<object, string[]>();
const keyIndexCache = new WeakMap<object, Map<string, number>>();

export function keysOf(value: Record<string, unknown>): string[] {
  let keys = keyCache.get(value);
  if (!keys) {
    keys = Object.keys(value);
    keyCache.set(value, keys);
  }
  return keys;
}

export function childCount(value: unknown): number {
  if (Array.isArray(value)) return value.length;
  if (isBranch(value)) return keysOf(value as Record<string, unknown>).length;
  return 0;
}

export function childAt(branch: Branch, index: number): [PathSegment, unknown] {
  if (Array.isArray(branch)) return [index, branch[index]];
  const key = keysOf(branch)[index];
  return [key, branch[key]];
}

function indexOfSegment(branch: Branch, segment: PathSegment): number {
  if (Array.isArray(branch)) return typeof segment === 'number' ? segment : -1;
  let index = keyIndexCache.get(branch);
  if (!index) {
    index = new Map(keysOf(branch).map((key, i) => [key, i]));
    keyIndexCache.set(branch, index);
  }
  return index.get(String(segment)) ?? -1;
}

// ---------------------------------------------------------------------------
// Groups for wide branches
// ---------------------------------------------------------------------------

/**
 * Size of each group when a branch (or a group range) has `count` children,
 * or 0 when it is small enough to list flat. Groups nest: the size grows by
 * CHUNK_SIZE until there are at most CHUNK_SIZE groups at each level, so a
 * million-item array opens as 100 groups of 10,000, each holding 100 groups
 * of 100.
 */
export function groupSize(count: number): number {
  if (count <= CHUNK_SIZE) return 0;
  let size = CHUNK_SIZE;
  while (Math.ceil(count / size) > CHUNK_SIZE) size *= CHUNK_SIZE;
  return size;
}

/** Row id of a group. Path keys never contain `#` outside quotes. */
export function groupId(
  branchPath: string,
  start: number,
  end: number
): string {
  return `${branchPath}#${start}-${end - 1}`;
}

/** The group ids that contain child `index` of a branch with `count` children. */
function groupChain(
  branchPath: string,
  count: number,
  index: number
): string[] {
  const chain: string[] = [];
  let start = 0;
  let end = count;
  for (let size = groupSize(end - start); size; size = groupSize(end - start)) {
    const groupStart = start + Math.floor((index - start) / size) * size;
    const groupEnd = Math.min(groupStart + size, end);
    chain.push(groupId(branchPath, groupStart, groupEnd));
    start = groupStart;
    end = groupEnd;
  }
  return chain;
}

/**
 * Ids that must be expanded for the node at `segments` to be visible: every
 * ancestor branch and every group on the way. Excludes the node itself.
 */
export function ancestorIds(
  root: unknown,
  segments: readonly PathSegment[]
): string[] {
  const ids: string[] = [];
  let branch: unknown = root;
  let path = '';
  segments.forEach((segment, depth) => {
    if (!isBranch(branch)) return;
    const index = indexOfSegment(branch, segment);
    if (index >= 0) ids.push(...groupChain(path, childCount(branch), index));
    path = appendPath(path, segment);
    if (depth < segments.length - 1) ids.push(path);
    branch = (branch as Record<string | number, unknown>)[segment];
  });
  return ids;
}

// ---------------------------------------------------------------------------
// Flattening into rows
// ---------------------------------------------------------------------------

export interface TreeRow {
  /** Stable id: the node's path, or a group id. */
  id: string;
  depth: number;
  type: 'node' | 'group';
  /** Node: its path. Group: the path of the branch it belongs to. */
  path: string;
  /** Node: its segments. Group: the owning branch's segments. */
  segments: readonly PathSegment[];
  /** Object key or array index; null for a bare root value and for groups. */
  label: PathSegment | null;
  /** Groups: [start, end) child indices of the owning branch. */
  range?: readonly [number, number];
  /** Node: its value. Group: the owning branch. */
  value: unknown;
  kind: JsonKind;
  /** Branches: children. Groups: children in the range. */
  childCount: number;
  expandable: boolean;
  expanded: boolean;
}

/** Restricts flattening to search hits, their ancestors and their subtrees. */
export interface RowFilter {
  /** Ids of matches and of every branch or group on the way to one. */
  relevant: ReadonlySet<string>;
  /** Paths of the matches themselves; their subtrees stay fully visible. */
  matched: ReadonlySet<string>;
}

export function rootRow(root: unknown): TreeRow {
  return {
    id: '',
    depth: 0,
    type: 'node',
    path: '',
    segments: [],
    label: null,
    value: root,
    kind: kindOf(root),
    childCount: 0,
    expandable: false,
    expanded: false,
  };
}

/**
 * The rows to show: the root's children, plus the children of every expanded
 * branch or group. A non-branch root is a single row.
 */
export function flattenRows(
  root: unknown,
  expanded: ReadonlySet<string>,
  filter?: RowFilter | null
): TreeRow[] {
  if (root === undefined) return [];
  if (!isBranch(root)) return [rootRow(root)];
  const rows: TreeRow[] = [];

  const pushRange = (
    branch: Branch,
    branchPath: string,
    segments: readonly PathSegment[],
    depth: number,
    start: number,
    end: number,
    unfiltered: boolean
  ) => {
    const size = groupSize(end - start);
    if (size) {
      for (let groupStart = start; groupStart < end; groupStart += size) {
        const groupEnd = Math.min(groupStart + size, end);
        const id = groupId(branchPath, groupStart, groupEnd);
        if (!unfiltered && filter && !filter.relevant.has(id)) continue;
        const open = expanded.has(id);
        rows.push({
          id,
          depth,
          type: 'group',
          path: branchPath,
          segments,
          label: null,
          range: [groupStart, groupEnd],
          value: branch,
          kind: kindOf(branch),
          childCount: groupEnd - groupStart,
          expandable: true,
          expanded: open,
        });
        if (open) {
          pushRange(
            branch,
            branchPath,
            segments,
            depth + 1,
            groupStart,
            groupEnd,
            unfiltered
          );
        }
      }
      return;
    }
    for (let i = start; i < end; i++) {
      const [segment, value] = childAt(branch, i);
      const path = appendPath(branchPath, segment);
      if (!unfiltered && filter && !filter.relevant.has(path)) continue;
      const branchChild = isBranch(value);
      const count = branchChild ? childCount(value) : 0;
      const open = branchChild && count > 0 && expanded.has(path);
      const childSegments = [...segments, segment];
      rows.push({
        id: path,
        depth,
        type: 'node',
        path,
        segments: childSegments,
        label: segment,
        value,
        kind: kindOf(value),
        childCount: count,
        expandable: branchChild && count > 0,
        expanded: open,
      });
      if (open) {
        pushRange(
          value as Branch,
          path,
          childSegments,
          depth + 1,
          0,
          count,
          unfiltered || (filter?.matched.has(path) ?? false)
        );
      }
    }
  };

  pushRange(root, '', [], 0, 0, childCount(root), !filter);
  return rows;
}

/**
 * Branch and group ids to open for "Expand all", breadth first so the top
 * levels open before the cap is reached.
 */
export function collectBranchIds(
  root: unknown,
  limit = EXPAND_ALL_MAX_BRANCHES
): { ids: string[]; truncated: boolean } {
  const ids: string[] = [];
  if (!isBranch(root)) return { ids, truncated: false };
  type Item = { branch: Branch; path: string; start: number; end: number };
  const queue: Item[] = [
    { branch: root, path: '', start: 0, end: childCount(root) },
  ];
  for (let head = 0; head < queue.length; head++) {
    const { branch, path, start, end } = queue[head];
    const size = groupSize(end - start);
    if (size) {
      for (let s = start; s < end; s += size) {
        const e = Math.min(s + size, end);
        if (ids.length >= limit) return { ids, truncated: true };
        ids.push(groupId(path, s, e));
        queue.push({ branch, path, start: s, end: e });
      }
      continue;
    }
    for (let i = start; i < end; i++) {
      const [segment, value] = childAt(branch, i);
      if (!isBranch(value) || childCount(value) === 0) continue;
      if (ids.length >= limit) return { ids, truncated: true };
      const childPath = appendPath(path, segment);
      ids.push(childPath);
      queue.push({
        branch: value,
        path: childPath,
        start: 0,
        end: childCount(value),
      });
    }
  }
  return { ids, truncated: false };
}

/**
 * Branches to open on first render: when the root has a handful of keys, open
 * each small top-level branch so the reader sees one level of detail.
 */
export function initialExpanded(root: unknown): Set<string> {
  const open = new Set<string>();
  if (!isBranch(root) || childCount(root) > 10) return open;
  for (let i = 0; i < childCount(root); i++) {
    const [segment, value] = childAt(root, i);
    const count = childCount(value);
    if (isBranch(value) && count > 0 && count <= 20)
      open.add(appendPath('', segment));
  }
  return open;
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

export interface SearchMatch {
  path: string;
  segments: readonly PathSegment[];
}

export interface SearchResult {
  query: string;
  matches: SearchMatch[];
  /** Fields visited. */
  scanned: number;
  /** Why the walk stopped early, or null when it covered everything. */
  stoppedAt: 'matches' | 'fields' | null;
}

export const EMPTY_SEARCH: SearchResult = {
  query: '',
  matches: [],
  scanned: 0,
  stoppedAt: null,
};

function primitiveText(value: unknown): string | null {
  // Elided stubs also carry `_truncated` and a preview; only true
  // truncations are searchable, since an elided value can be loaded whole.
  if (isElidedStub(value)) return null;
  if (isTruncatedPayload(value)) value = value._preview;
  switch (typeof value) {
    case 'string':
      return value.length > SEARCH_STRING_SCAN_LIMIT
        ? value.slice(0, SEARCH_STRING_SCAN_LIMIT)
        : value;
    case 'number':
    case 'boolean':
      return String(value);
    default:
      return null;
  }
}

/**
 * Case-insensitive search over object keys and primitive values, in document
 * order. Array indices, nulls and elided placeholders never match; the
 * stored preview of a server-truncated value does.
 */
export function searchPayload(
  root: unknown,
  rawQuery: string,
  limits: { maxMatches?: number; maxNodes?: number } = {}
): SearchResult {
  const query = rawQuery.trim().toLowerCase();
  if (!query || root === undefined) return { ...EMPTY_SEARCH, query };
  const maxMatches = limits.maxMatches ?? SEARCH_MAX_MATCHES;
  const maxNodes = limits.maxNodes ?? SEARCH_MAX_NODES;
  const matches: SearchMatch[] = [];
  let scanned = 0;

  type Frame = { value: unknown; segments: PathSegment[]; key: string | null };
  const stack: Frame[] = [{ value: root, segments: [], key: null }];
  while (stack.length) {
    if (scanned >= maxNodes) {
      return { query, matches, scanned, stoppedAt: 'fields' };
    }
    const { value, segments, key } = stack.pop()!;
    scanned++;
    const keyHit = key !== null && key.toLowerCase().includes(query);
    const text = keyHit ? null : primitiveText(value);
    if (keyHit || (text !== null && text.toLowerCase().includes(query))) {
      matches.push({ path: formatPath(segments), segments });
      if (matches.length >= maxMatches) {
        return { query, matches, scanned, stoppedAt: 'matches' };
      }
    }
    if (isBranch(value)) {
      // Push in reverse so the stack pops children in document order.
      for (let i = childCount(value) - 1; i >= 0; i--) {
        const [segment, child] = childAt(value, i);
        stack.push({
          value: child,
          segments: [...segments, segment],
          key: typeof segment === 'string' ? segment : null,
        });
      }
    }
  }
  return { query, matches, scanned, stoppedAt: null };
}

/** The filter for "Only matches": hits, everything on the way, their subtrees. */
export function matchFilter(
  root: unknown,
  matches: readonly SearchMatch[]
): RowFilter {
  const relevant = new Set<string>();
  const matched = new Set<string>();
  for (const match of matches) {
    matched.add(match.path);
    relevant.add(match.path);
    for (const id of ancestorIds(root, match.segments)) relevant.add(id);
  }
  return { relevant, matched };
}

// ---------------------------------------------------------------------------
// Size
// ---------------------------------------------------------------------------

export interface PayloadSize {
  /** Approximate compact-JSON size in bytes, elided values excluded. */
  bytes: number;
  /** Original size of values the server elided (full value fetchable). */
  elidedBytes: number;
  elidedCount: number;
  /** Original size of values the server cut to a preview (not fetchable). */
  truncatedBytes: number;
  truncatedCount: number;
  fields: number;
  /** False when the walk hit its budget; sizes are then lower bounds. */
  complete: boolean;
}

/**
 * Estimates the compact JSON size without serializing. Escapes are not
 * counted, so the figure is a slight underestimate for escape-heavy text.
 */
export function measurePayload(
  root: unknown,
  maxNodes = MEASURE_MAX_NODES
): PayloadSize {
  const size: PayloadSize = {
    bytes: 0,
    elidedBytes: 0,
    elidedCount: 0,
    truncatedBytes: 0,
    truncatedCount: 0,
    fields: 0,
    complete: true,
  };
  if (root === undefined) return size;
  const stack: unknown[] = [root];
  while (stack.length) {
    if (size.fields >= maxNodes) {
      size.complete = false;
      return size;
    }
    const value = stack.pop();
    size.fields++;
    if (isElidedStub(value)) {
      size.elidedCount++;
      size.elidedBytes += value._original_size;
      continue;
    }
    if (isTruncatedPayload(value)) {
      size.truncatedCount++;
      size.truncatedBytes += value._original_size;
      continue;
    }
    if (Array.isArray(value)) {
      size.bytes += 2 + Math.max(0, value.length - 1);
      for (let i = value.length - 1; i >= 0; i--) stack.push(value[i]);
      continue;
    }
    switch (typeof value) {
      case 'object': {
        if (value === null) {
          size.bytes += 4;
          break;
        }
        const keys = keysOf(value as Record<string, unknown>);
        size.bytes += 2 + Math.max(0, keys.length - 1);
        for (let i = keys.length - 1; i >= 0; i--) {
          size.bytes += keys[i].length + 3;
          stack.push((value as Record<string, unknown>)[keys[i]]);
        }
        break;
      }
      case 'string':
        size.bytes += value.length + 2;
        break;
      case 'number':
        size.bytes += String(value).length;
        break;
      case 'boolean':
        size.bytes += value ? 4 : 5;
        break;
      default:
        break;
    }
  }
  return size;
}

// ---------------------------------------------------------------------------
// Display helpers
// ---------------------------------------------------------------------------

export function truncateText(
  text: string,
  limit: number
): { text: string; truncated: boolean } {
  return text.length > limit
    ? { text: text.slice(0, limit), truncated: true }
    : { text, truncated: false };
}

/** Short inline preview of a collapsed branch: `{ a, b, c, … }` or `[ 1, 2, … ]`. */
export function branchPreview(value: unknown, maxItems = 4): string {
  if (Array.isArray(value)) {
    const shown = value.slice(0, maxItems);
    if (
      !shown.length ||
      !shown.every((item) =>
        ['string', 'number', 'boolean', 'null'].includes(kindOf(item))
      )
    )
      return '';
    const items = shown.map((item) =>
      typeof item === 'string'
        ? JSON.stringify(truncateText(item, 24).text)
        : String(item)
    );
    return `[ ${items.join(', ')}${value.length > maxItems ? ', …' : ''} ]`;
  }
  if (!isBranch(value)) return '';
  const keys = keysOf(value as Record<string, unknown>);
  if (!keys.length) return '';
  const shown = keys
    .slice(0, maxItems)
    .map((key) => truncateText(key, 24).text);
  return `{ ${shown.join(', ')}${keys.length > maxItems ? ', …' : ''} }`;
}

const countFormat = new Intl.NumberFormat('en-US');
export function formatCount(n: number): string {
  return countFormat.format(n);
}

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export function plural(n: number, one: string, many = `${one}s`): string {
  return `${formatCount(n)} ${n === 1 ? one : many}`;
}

/** "object · 3 keys", "string · 1,204 chars", "large value · 1.8 MB". */
export function describeValue(value: unknown): string {
  switch (kindOf(value)) {
    case 'object':
      return `object · ${plural(childCount(value), 'key')}`;
    case 'array':
      return `array · ${plural(childCount(value), 'item')}`;
    case 'string':
      return `string · ${plural((value as string).length, 'char')}`;
    case 'elided':
      return `large value · ${formatSize((value as { _original_size: number })._original_size)}`;
    case 'truncated':
      return `cut off · ${formatSize((value as { _original_size: number })._original_size)} originally`;
    case 'number':
      return 'number';
    case 'boolean':
      return 'boolean';
    case 'null':
      return 'null';
    default:
      return typeof value;
  }
}

/** Items of a group, as a value to copy: an array slice or an object subset. */
export function groupValue(
  branch: unknown,
  range: readonly [number, number]
): unknown {
  if (Array.isArray(branch)) return branch.slice(range[0], range[1]);
  if (!isBranch(branch)) return undefined;
  const out: Record<string, unknown> = {};
  for (let i = range[0]; i < range[1]; i++) {
    const [key, value] = childAt(branch, i);
    out[key as string] = value;
  }
  return out;
}

export type RawLines =
  | { status: 'ok'; lines: string[] }
  | { status: 'too-large'; size: PayloadSize };

/** Pretty-printed JSON split into lines, unless the payload is too large. */
export function rawLines(
  root: unknown,
  maxBytes = RAW_VIEW_MAX_BYTES
): RawLines {
  if (root === undefined) return { status: 'ok', lines: [] };
  const size = measurePayload(root);
  if (!size.complete || size.bytes > maxBytes)
    return { status: 'too-large', size };
  return { status: 'ok', lines: JSON.stringify(root, null, 2).split('\n') };
}
