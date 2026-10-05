import { describe, expect, it } from 'vitest';
import { appendPathSegment, referenceSegments } from './reference-path';

describe('appendPathSegment', () => {
  it('writes plain names in dotted form', () => {
    expect(appendPathSegment('data', 'orderId')).toBe('data.orderId');
    expect(appendPathSegment('data', 'order_id')).toBe('data.order_id');
    expect(appendPathSegment('data', 'order-id')).toBe('data.order-id');
    expect(appendPathSegment('data', '0')).toBe('data.0');
    expect(appendPathSegment("steps['fetch'].outputs", 'body')).toBe(
      "steps['fetch'].outputs.body"
    );
  });

  it('bracket-quotes names the dotted form would split or reject', () => {
    expect(appendPathSegment('data', 'a.b')).toBe('data["a.b"]');
    expect(appendPathSegment('data', 'a]b')).toBe('data["a]b"]');
    expect(appendPathSegment('data', 'a[0]')).toBe('data["a[0]"]');
    expect(appendPathSegment('data', 'first name')).toBe('data["first name"]');
    expect(appendPathSegment("steps['fetch'].outputs", 'a.b')).toBe(
      `steps['fetch'].outputs["a.b"]`
    );
  });

  it('quotes with the other quote character when the name contains one', () => {
    expect(appendPathSegment('data', 'say "hi"')).toBe(`data['say "hi"']`);
    expect(appendPathSegment('data', "it's")).toBe(`data["it's"]`);
  });

  it('falls back to dotted form for a name with both quotes and no structure', () => {
    expect(appendPathSegment('data', `it's "x"`)).toBe(`data.it's "x"`);
  });

  it('emits a malformed path when no spelling reads back as the name', () => {
    // Both quote characters plus a dot: the validator rejects the result
    // instead of resolving it to some other key.
    expect(appendPathSegment('data', `a.b'"`)).toBe(`data["a.b'""]`);
    expect(appendPathSegment('data', '')).toBe('data[""]');
  });

  it('round-trips through the tokenizer for every name it can spell', () => {
    const names = [
      'plain',
      'order-id',
      '0',
      '-1',
      'a.b',
      'a..b',
      'a]b',
      'a[0]',
      '[x]',
      '.',
      ']',
      '"',
      "'",
      ' padded ',
      'first name',
      'ünïcode',
      'say "hi"',
      "it's",
      `it's "x"`,
      'a.b[0]',
    ];
    for (const name of names) {
      expect(referenceSegments(appendPathSegment('data', name))).toEqual([
        'data',
        name,
      ]);
      expect(
        referenceSegments(appendPathSegment("steps['s'].outputs", name))
      ).toEqual(['steps', 's', 'outputs', name]);
    }
  });
});

describe('referenceSegments', () => {
  it('splits on dots and drops empty segments', () => {
    expect(referenceSegments('data.order.id')).toEqual(['data', 'order', 'id']);
    expect(referenceSegments('data')).toEqual(['data']);
    expect(referenceSegments('')).toEqual([]);
    expect(referenceSegments('data..order')).toEqual(['data', 'order']);
    expect(referenceSegments('data.')).toEqual(['data']);
    expect(referenceSegments('.data.a')).toEqual(['data', 'a']);
  });

  it('reads a quoted bracket body as one key, up to its closing quote', () => {
    expect(referenceSegments(`data["a.b"]`)).toEqual(['data', 'a.b']);
    expect(referenceSegments(`data['a.b']`)).toEqual(['data', 'a.b']);
    expect(referenceSegments(`data["a..b"]`)).toEqual(['data', 'a..b']);
    expect(referenceSegments(`data["a]b"]`)).toEqual(['data', 'a]b']);
    expect(referenceSegments(`data["]"]`)).toEqual(['data', ']']);
    expect(referenceSegments(`data["a[0]"].b`)).toEqual(['data', 'a[0]', 'b']);
    expect(referenceSegments(`data[ "a]b" ].c`)).toEqual(['data', 'a]b', 'c']);
    expect(referenceSegments(`data['say "hi"']`)).toEqual(['data', 'say "hi"']);
    expect(referenceSegments(`data[" a ]"]`)).toEqual(['data', ' a ]']);
    expect(referenceSegments(`steps['fetch'].outputs`)).toEqual([
      'steps',
      'fetch',
      'outputs',
    ]);
  });

  it('reads an unquoted bracket body up to the first ], trimmed', () => {
    expect(referenceSegments('items[0]')).toEqual(['items', '0']);
    expect(referenceSegments('items[-1]')).toEqual(['items', '-1']);
    expect(referenceSegments('foo[ bar ]')).toEqual(['foo', 'bar']);
    expect(referenceSegments(`a["b.c"][0].d`)).toEqual(['a', 'b.c', '0', 'd']);
  });

  it('splits malformed paths the way the backend does', () => {
    expect(referenceSegments('data[]')).toEqual(['data']);
    expect(referenceSegments(`data[""]`)).toEqual(['data']);
    expect(referenceSegments('data.a]')).toEqual(['data', 'a]']);
    expect(referenceSegments('data[a..b')).toEqual(['data', 'a..b']);
    expect(referenceSegments(`foo['a"]`)).toEqual(['foo', `'a"`]);
    expect(referenceSegments(`foo["]`)).toEqual(['foo', '"']);
  });
});
