import { describe, expect, it } from 'vitest';

import { matchFolders } from './folder-search';

const folders = [
  { name: 'Commerce' },
  { name: 'Customer' },
  { name: 'Microsoft Azure' },
];

describe('matchFolders', () => {
  it('keeps every folder when the term is blank', () => {
    expect(matchFolders(folders, '')).toBe(folders);
    expect(matchFolders(folders, '   ')).toBe(folders);
  });

  it('matches a case-insensitive substring of the name', () => {
    expect(matchFolders(folders, 'cust')).toEqual([{ name: 'Customer' }]);
    expect(matchFolders(folders, 'AZURE')).toEqual([
      { name: 'Microsoft Azure' },
    ]);
    expect(matchFolders(folders, 'c')).toHaveLength(3);
  });

  it('ignores surrounding whitespace, like the workflow search does', () => {
    expect(matchFolders(folders, '  commerce  ')).toEqual([
      { name: 'Commerce' },
    ]);
  });

  it('returns nothing when the term matches no folder', () => {
    expect(matchFolders(folders, 'zzzznomatch')).toEqual([]);
  });
});
