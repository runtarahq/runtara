import { describe, expect, it } from 'vitest';
import {
  CHUNK_SIZE,
  EXPAND_ALL_MAX_BRANCHES,
  RAW_VIEW_MAX_BYTES,
  ancestorIds,
  appendPath,
  branchPreview,
  collectBranchIds,
  describeValue,
  flattenRows,
  formatPath,
  formatSize,
  getAt,
  groupId,
  groupSize,
  groupValue,
  initialExpanded,
  kindOf,
  matchFilter,
  measurePayload,
  rawLines,
  searchPayload,
  truncateText,
} from './model';

const elided = {
  _elided: true,
  _original_size: 1_843_200,
  _preview: 'JVBERi0x',
};

/** A run output of roughly `bytes` size: an array of product records. */
function bigPayload(items: number) {
  return {
    feeds_seen: items,
    enriched: Array.from({ length: items }, (_, i) => ({
      sku: `SKU-${String(i).padStart(6, '0')}`,
      title: `Industrial part number ${i} with a reasonably long description`,
      seo_score: i % 100,
      market_price: { min: 1.5, median: 2.25, max: 3.75, currency: 'EUR' },
      keywords: ['part', `kw-${i % 37}`],
    })),
  };
}

describe('kindOf', () => {
  it('classifies JSON values, telling elided placeholders apart from objects', () => {
    expect(kindOf({})).toBe('object');
    expect(kindOf([])).toBe('array');
    expect(kindOf('x')).toBe('string');
    expect(kindOf(1)).toBe('number');
    expect(kindOf(false)).toBe('boolean');
    expect(kindOf(null)).toBe('null');
    expect(kindOf(elided)).toBe('elided');
    expect(kindOf(undefined)).toBe('unknown');
  });
});

describe('paths', () => {
  it('formats identifiers with dots, indices with brackets and odd keys quoted', () => {
    expect(formatPath(['enriched', 0, 'market_price', 'median'])).toBe(
      'enriched[0].market_price.median'
    );
    expect(formatPath(['headers', 'content-type'])).toBe(
      'headers["content-type"]'
    );
    expect(formatPath(['a "quoted" key'])).toBe('["a \\"quoted\\" key"]');
    expect(formatPath([3])).toBe('[3]');
    expect(appendPath('', '$ref')).toBe('$ref');
  });

  it('reads the value at a path', () => {
    const value = { a: [{ 'b-c': 4 }] };
    expect(getAt(value, ['a', 0, 'b-c'])).toBe(4);
    expect(getAt(value, [])).toBe(value);
    expect(getAt(value, ['a', 5, 'x'])).toBeUndefined();
    expect(getAt('text', ['length'])).toBeUndefined();
  });
});

describe('groupSize', () => {
  it('lists small branches flat and nests groups for wide ones', () => {
    expect(groupSize(0)).toBe(0);
    expect(groupSize(CHUNK_SIZE)).toBe(0);
    expect(groupSize(CHUNK_SIZE + 1)).toBe(CHUNK_SIZE);
    expect(groupSize(10_000)).toBe(100);
    expect(groupSize(10_001)).toBe(10_000);
    expect(groupSize(1_000_000)).toBe(10_000);
    expect(groupSize(1_000_001)).toBe(1_000_000);
  });
});

describe('flattenRows', () => {
  const value = {
    feeds_seen: 14,
    enriched: [{ sku: 'ACM-1', price: { min: 1 } }],
    updated_at: '2026-10-11T07:02:37Z',
  };

  it('shows only the top level when nothing is expanded', () => {
    const rows = flattenRows(value, new Set());
    expect(rows.map((r) => r.id)).toEqual([
      'feeds_seen',
      'enriched',
      'updated_at',
    ]);
    expect(rows[1]).toMatchObject({
      kind: 'array',
      childCount: 1,
      expandable: true,
      expanded: false,
    });
    expect(rows[0]).toMatchObject({
      kind: 'number',
      expandable: false,
      depth: 0,
    });
  });

  it('includes the children of expanded branches, depth first', () => {
    const rows = flattenRows(value, new Set(['enriched', 'enriched[0]']));
    expect(rows.map((r) => [r.id, r.depth])).toEqual([
      ['feeds_seen', 0],
      ['enriched', 0],
      ['enriched[0]', 1],
      ['enriched[0].sku', 2],
      ['enriched[0].price', 2],
      ['updated_at', 0],
    ]);
    expect(rows[3].segments).toEqual(['enriched', 0, 'sku']);
  });

  it('turns a primitive or elided root into a single row', () => {
    expect(flattenRows('done', new Set())).toEqual([
      expect.objectContaining({
        id: '',
        label: null,
        kind: 'string',
        value: 'done',
      }),
    ]);
    expect(flattenRows(elided, new Set())[0].kind).toBe('elided');
    expect(flattenRows(undefined, new Set())).toEqual([]);
  });

  it('marks empty branches as not expandable', () => {
    const rows = flattenRows({ skipped: [], meta: {} }, new Set(['skipped']));
    expect(rows.map((r) => [r.id, r.expandable, r.expanded])).toEqual([
      ['skipped', false, false],
      ['meta', false, false],
    ]);
  });

  it('splits a wide array into groups of 100 and opens only the expanded group', () => {
    const wide = { items: Array.from({ length: 250 }, (_, i) => i) };
    const collapsedGroups = flattenRows(wide, new Set(['items']));
    expect(collapsedGroups.map((r) => r.id)).toEqual([
      'items',
      groupId('items', 0, 100),
      groupId('items', 100, 200),
      groupId('items', 200, 250),
    ]);
    expect(collapsedGroups[3]).toMatchObject({
      type: 'group',
      range: [200, 250],
      childCount: 50,
    });

    const opened = flattenRows(wide, new Set(['items', 'items#100-199']));
    expect(opened).toHaveLength(4 + 100);
    expect(opened[3]).toMatchObject({ id: 'items[100]', depth: 2, value: 100 });
  });

  it('nests groups for very wide branches so no level lists more than 100 rows', () => {
    const huge = Array.from({ length: 1_000_000 }, (_, i) => i);
    const top = flattenRows(huge, new Set());
    expect(top).toHaveLength(100);
    expect(top[0]).toMatchObject({ id: '#0-9999', childCount: 10_000 });
    const second = flattenRows(huge, new Set(['#0-9999']));
    expect(second).toHaveLength(200);
    expect(second[1]).toMatchObject({ id: '#0-99', depth: 1 });
  });

  it('groups wide objects by key position', () => {
    const wide = Object.fromEntries(
      Array.from({ length: 150 }, (_, i) => [`k${i}`, i])
    );
    const rows = flattenRows(wide, new Set(['#100-149']));
    expect(rows.map((r) => r.id).slice(0, 3)).toEqual([
      '#0-99',
      '#100-149',
      'k100',
    ]);
  });

  it('keeps only matches, their ancestors and their subtrees when filtered', () => {
    const payload = {
      a: { keep: 1, drop: 2 },
      b: { nested: { deep: 'x' } },
      c: 3,
    };
    const result = searchPayload(payload, 'keep');
    const filter = matchFilter(payload, result.matches);
    const rows = flattenRows(payload, new Set(['a', 'b']), filter);
    expect(rows.map((r) => r.id)).toEqual(['a', 'a.keep']);

    const branchHit = searchPayload(payload, 'b');
    const branchRows = flattenRows(
      payload,
      new Set(['b', 'b.nested']),
      matchFilter(payload, branchHit.matches)
    );
    expect(branchRows.map((r) => r.id)).toEqual([
      'b',
      'b.nested',
      'b.nested.deep',
    ]);
  });

  it('flattens a payload over 1 MB in well under a frame budget once expanded', () => {
    const payload = bigPayload(10_000);
    expect(measurePayload(payload).bytes).toBeGreaterThan(1_000_000);
    const started = performance.now();
    const rows = flattenRows(
      payload,
      new Set(['enriched', 'enriched#0-99', 'enriched[0]'])
    );
    const elapsed = performance.now() - started;
    expect(rows.length).toBeLessThan(250);
    expect(elapsed).toBeLessThan(50);
  });
});

describe('ancestorIds', () => {
  it('lists every branch and group on the way to a node', () => {
    const value = { items: Array.from({ length: 250 }, (_, i) => ({ id: i })) };
    expect(ancestorIds(value, ['items', 120, 'id'])).toEqual([
      'items',
      'items#100-199',
      'items[120]',
    ]);
  });

  it('includes nested groups and works for wide objects', () => {
    const huge = Array.from({ length: 20_000 }, (_, i) => i);
    expect(ancestorIds(huge, [12_345])).toEqual([
      '#10000-19999',
      '#12300-12399',
    ]);
    const wide = Object.fromEntries(
      Array.from({ length: 150 }, (_, i) => [`k${i}`, i])
    );
    expect(ancestorIds(wide, ['k120'])).toEqual(['#100-149']);
  });

  it('reveals the node when expanded', () => {
    const value = { items: Array.from({ length: 250 }, (_, i) => ({ id: i })) };
    const rows = flattenRows(
      value,
      new Set(ancestorIds(value, ['items', 120, 'id']))
    );
    expect(rows.some((r) => r.id === 'items[120].id')).toBe(true);
  });
});

describe('searchPayload', () => {
  const value = {
    Customer: { email: 'Ada@Example.com', tags: ['vip', 'beta'] },
    total: 4250,
    paid: true,
    note: null,
    file: elided,
  };

  it('matches keys and primitive values case-insensitively, in document order', () => {
    expect(searchPayload(value, 'customer').matches.map((m) => m.path)).toEqual(
      ['Customer']
    );
    expect(searchPayload(value, 'EXAMPLE').matches.map((m) => m.path)).toEqual([
      'Customer.email',
    ]);
    expect(searchPayload(value, '425').matches.map((m) => m.path)).toEqual([
      'total',
    ]);
    expect(searchPayload(value, 'true').matches.map((m) => m.path)).toEqual([
      'paid',
    ]);
    expect(searchPayload(value, 'a').matches.map((m) => m.path)).toEqual([
      'Customer.email',
      'Customer.tags',
      'Customer.tags[1]',
      'total',
      'paid',
    ]);
  });

  it('never matches array indices, nulls or elided placeholders', () => {
    expect(searchPayload(value, '1').matches).toEqual([]);
    expect(searchPayload(value, 'null').matches).toEqual([]);
    expect(searchPayload(value, 'JVBER').matches).toEqual([]);
  });

  it('returns nothing for a blank query', () => {
    expect(searchPayload(value, '   ')).toMatchObject({
      matches: [],
      scanned: 0,
      stoppedAt: null,
    });
  });

  it('stops at the match cap and says so', () => {
    const many = Array.from({ length: 5_000 }, () => 'hit');
    const result = searchPayload(many, 'hit', { maxMatches: 100 });
    expect(result.matches).toHaveLength(100);
    expect(result.stoppedAt).toBe('matches');
    expect(result.matches[99].path).toBe('[99]');
  });

  it('stops at the field budget and says so', () => {
    const many = Array.from({ length: 5_000 }, () => 'miss');
    const result = searchPayload(many, 'hit', { maxNodes: 1_000 });
    expect(result).toMatchObject({
      matches: [],
      scanned: 1_000,
      stoppedAt: 'fields',
    });
  });

  it('searches only the head of very long strings', () => {
    const long = 'a'.repeat(200_000) + 'needle';
    expect(searchPayload({ blob: long }, 'needle').matches).toEqual([]);
    expect(
      searchPayload({ blob: 'needle' + long }, 'needle').matches
    ).toHaveLength(1);
  });

  it('searches a payload over 1 MB completely and quickly', () => {
    const payload = bigPayload(10_000);
    const started = performance.now();
    const result = searchPayload(payload, 'SKU-009999');
    const elapsed = performance.now() - started;
    expect(result.stoppedAt).toBeNull();
    expect(result.matches.map((m) => m.path)).toEqual(['enriched[9999].sku']);
    expect(elapsed).toBeLessThan(1_500);
  });
});

describe('collectBranchIds', () => {
  it('collects branches and groups breadth first', () => {
    const value = {
      a: { b: { c: 1 } },
      list: Array.from({ length: 150 }, (_, i) => ({ i })),
    };
    const { ids, truncated } = collectBranchIds(value);
    expect(truncated).toBe(false);
    expect(ids.slice(0, 4)).toEqual(['a', 'list', 'a.b', 'list#0-99']);
    expect(ids).toContain('list[149]');
  });

  it('stops at the cap', () => {
    const value = Array.from({ length: 5_000 }, (_, i) => ({ i }));
    const { ids, truncated } = collectBranchIds(value, 50);
    expect(ids).toHaveLength(50);
    expect(truncated).toBe(true);
    expect(collectBranchIds(value).ids).toHaveLength(EXPAND_ALL_MAX_BRANCHES);
  });
});

describe('initialExpanded', () => {
  it('opens small top-level branches of a small root only', () => {
    expect([
      ...initialExpanded({ data: { a: 1 }, variables: {}, n: 1 }),
    ]).toEqual(['data']);
    const wideRoot = Object.fromEntries(
      Array.from({ length: 11 }, (_, i) => [`k${i}`, { a: 1 }])
    );
    expect(initialExpanded(wideRoot).size).toBe(0);
    expect(
      initialExpanded({ big: Array.from({ length: 21 }, (_, i) => i) }).size
    ).toBe(0);
    expect(initialExpanded('text').size).toBe(0);
  });
});

describe('measurePayload', () => {
  it('estimates compact JSON size exactly for escape-free payloads', () => {
    const value = { a: [1, 'two', true, null], b: { c: 'd' } };
    expect(measurePayload(value)).toMatchObject({
      bytes: JSON.stringify(value).length,
      elidedCount: 0,
      complete: true,
    });
  });

  it('counts elided values separately', () => {
    expect(measurePayload({ file: elided })).toMatchObject({
      elidedCount: 1,
      elidedBytes: 1_843_200,
    });
  });

  it('stops at its budget and reports a lower bound', () => {
    const size = measurePayload(
      Array.from({ length: 100 }, () => 1),
      10
    );
    expect(size).toMatchObject({ fields: 10, complete: false });
  });
});

describe('rawLines', () => {
  it('pretty-prints small payloads', () => {
    expect(rawLines({ a: 1 })).toEqual({
      status: 'ok',
      lines: ['{', '  "a": 1', '}'],
    });
    expect(rawLines(undefined)).toEqual({ status: 'ok', lines: [] });
  });

  it('refuses payloads over the cap without serializing them', () => {
    const result = rawLines({ blob: 'x'.repeat(100) }, 50);
    expect(result.status).toBe('too-large');
    expect(rawLines(bigPayload(20_000)).status).toBe(
      measurePayload(bigPayload(20_000)).bytes > RAW_VIEW_MAX_BYTES
        ? 'too-large'
        : 'ok'
    );
  });
});

describe('display helpers', () => {
  it('previews collapsed branches', () => {
    expect(branchPreview({ a: 1, b: 2, c: 3, d: 4, e: 5 })).toBe(
      '{ a, b, c, d, … }'
    );
    expect(branchPreview(['x', 2, true])).toBe('[ "x", 2, true ]');
    expect(branchPreview([{ a: 1 }])).toBe('');
    expect(branchPreview({})).toBe('');
  });

  it('describes values for the inspector', () => {
    expect(describeValue({ a: 1, b: 2 })).toBe('object · 2 keys');
    expect(describeValue([1])).toBe('array · 1 item');
    expect(describeValue('x'.repeat(1204))).toBe('string · 1,204 chars');
    expect(describeValue(elided)).toBe('large value · 1.8 MB');
    expect(describeValue(null)).toBe('null');
  });

  it('truncates text and formats sizes', () => {
    expect(truncateText('abcdef', 3)).toEqual({ text: 'abc', truncated: true });
    expect(truncateText('abc', 3)).toEqual({ text: 'abc', truncated: false });
    expect(formatSize(512)).toBe('512 B');
    expect(formatSize(2048)).toBe('2.0 KB');
    expect(formatSize(1_843_200)).toBe('1.8 MB');
  });

  it('copies a group as a slice of its branch', () => {
    expect(groupValue([0, 1, 2, 3], [1, 3])).toEqual([1, 2]);
    expect(groupValue({ a: 1, b: 2, c: 3 }, [1, 3])).toEqual({ b: 2, c: 3 });
  });
});

describe('server-truncated values', () => {
  const truncated = {
    _truncated: true,
    _original_size: 1_258_291,
    _preview: '{"items":[{"sku":"ACM-1"',
  };
  // Elided stubs as the server really sends them also carry `_truncated`.
  const realElided = {
    _truncated: true,
    _elided: true,
    _original_size: 32 * 1024,
    _preview: 'AAAA',
  };

  it('classifies truncations apart from loadable elided values', () => {
    expect(kindOf(truncated)).toBe('truncated');
    expect(kindOf(realElided)).toBe('elided');
    expect(flattenRows({ payload: truncated }, new Set())[0]).toMatchObject({
      kind: 'truncated',
      expandable: false,
    });
  });

  it('searches the stored preview of a truncation but not of an elided value', () => {
    expect(
      searchPayload({ payload: truncated }, 'acm-1').matches.map((m) => m.path)
    ).toEqual(['payload']);
    expect(searchPayload({ upload: realElided }, 'AAAA').matches).toEqual([]);
  });

  it('counts truncations separately when sizing', () => {
    expect(measurePayload({ a: truncated, b: realElided })).toMatchObject({
      truncatedCount: 1,
      truncatedBytes: 1_258_291,
      elidedCount: 1,
      elidedBytes: 32 * 1024,
    });
    expect(describeValue(truncated)).toBe('cut off · 1.2 MB originally');
  });
});
