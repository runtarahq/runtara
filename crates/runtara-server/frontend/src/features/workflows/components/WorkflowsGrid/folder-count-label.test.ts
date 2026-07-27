import { describe, expect, it } from 'vitest';

import { folderCountLabel } from './folder-count-label';

describe('folderCountLabel', () => {
  it('renders nothing while the count is still unknown', () => {
    // Not "0 workflows" — a pending count must never look like an empty folder.
    expect(folderCountLabel(undefined)).toBe('');
  });

  it('reports a genuine zero', () => {
    expect(folderCountLabel(0)).toBe('0 workflows');
  });

  it('uses the singular for exactly one', () => {
    expect(folderCountLabel(1)).toBe('1 workflow');
  });

  it('uses the plural beyond one', () => {
    expect(folderCountLabel(2)).toBe('2 workflows');
    expect(folderCountLabel(124)).toBe('124 workflows');
  });
});
