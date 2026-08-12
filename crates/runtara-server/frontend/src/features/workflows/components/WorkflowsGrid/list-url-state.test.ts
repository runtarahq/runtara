import { describe, expect, it } from 'vitest';

import {
  DEFAULT_PAGE_SIZE,
  readListUrlState,
  writeListUrlState,
} from './list-url-state.ts';

function read(query: string) {
  return readListUrlState(new URLSearchParams(query));
}

function write(query: string, patch: Parameters<typeof writeListUrlState>[1]) {
  return writeListUrlState(new URLSearchParams(query), patch).toString();
}

describe('readListUrlState', () => {
  it('falls back to the first page, the default size and no query', () => {
    expect(read('')).toEqual({
      page: 0,
      pageSize: DEFAULT_PAGE_SIZE,
      search: '',
    });
  });

  it('reads a 1-based page as a 0-based index', () => {
    expect(read('page=3').page).toBe(2);
    expect(read('page=1').page).toBe(0);
  });

  it('ignores page values the listing cannot use', () => {
    // Hand-edited URLs are input, not a promise.
    for (const query of [
      'page=0',
      'page=-4',
      'page=abc',
      'page=2.5',
      'page=',
    ]) {
      expect(read(query).page).toBe(0);
    }
  });

  it('accepts only the sizes the pagination control offers', () => {
    expect(read('pageSize=50').pageSize).toBe(50);
    for (const query of ['pageSize=7', 'pageSize=0', 'pageSize=nope']) {
      expect(read(query).pageSize).toBe(DEFAULT_PAGE_SIZE);
    }
  });

  it('reads the query verbatim, spaces and all', () => {
    expect(read('q=order+sync').search).toBe('order sync');
  });
});

describe('writeListUrlState', () => {
  it('leaves the default view without any listing parameters', () => {
    expect(write('page=4&pageSize=50&q=sync', { page: 0 })).toBe(
      'pageSize=50&q=sync'
    );
    expect(write('pageSize=50', { pageSize: DEFAULT_PAGE_SIZE })).toBe('');
    expect(write('q=sync', { search: '' })).toBe('');
    expect(write('q=sync', { search: '   ' })).toBe('');
  });

  it('writes the page 1-based', () => {
    expect(write('', { page: 2 })).toBe('page=3');
  });

  it('drops the page when the search changes', () => {
    // Page 4 of the old result set says nothing about the new one.
    expect(write('page=4&q=old', { search: 'new' })).toBe('q=new');
  });

  it('drops the page when the page size changes', () => {
    expect(write('page=4', { pageSize: 20 })).toBe('pageSize=20');
  });

  it('leaves parameters it was not asked about alone', () => {
    expect(write('folder=%2FDemo%2F&q=sync', { page: 1 })).toBe(
      'folder=%2FDemo%2F&q=sync&page=2'
    );
  });
});
