import { describe, expect, it } from 'vitest';
import {
  DRAFT_ID_PREFIX,
  computeRecordTotals,
  isDraftRow,
  makeDraftInstance,
} from './draft-row';

describe('isDraftRow', () => {
  it('recognizes draft ids and rejects server ids', () => {
    expect(isDraftRow(`${DRAFT_ID_PREFIX}1753600000000`)).toBe(true);
    expect(isDraftRow('550e8400-e29b-41d4-a716-446655440000')).toBe(false);
    expect(isDraftRow(undefined)).toBe(false);
    expect(isDraftRow(null)).toBe(false);
    expect(isDraftRow('')).toBe(false);
  });
});

describe('makeDraftInstance', () => {
  it('carries no fabricated timestamps and no properties', () => {
    const draft = makeDraftInstance(`${DRAFT_ID_PREFIX}1`);

    expect(draft.id).toBe(`${DRAFT_ID_PREFIX}1`);
    expect(draft.properties).toEqual({});
    expect(draft.createdAt).toBe('');
    expect(draft.updatedAt).toBe('');
  });
});

describe('computeRecordTotals', () => {
  it('reports an empty table as empty; drafts are not part of the input', () => {
    expect(
      computeRecordTotals({ content: [], totalPages: 0, totalElements: 0 }, 20)
    ).toEqual({ totalPages: 1, totalElements: 0 });
  });

  it('uses the API totals when present', () => {
    expect(
      computeRecordTotals(
        { content: new Array(20).fill({}), totalPages: 3, totalElements: 42 },
        20
      )
    ).toEqual({ totalPages: 3, totalElements: 42 });
  });

  it('derives the page count from totalElements when totalPages is absent', () => {
    expect(
      computeRecordTotals(
        { content: new Array(20).fill({}), totalPages: 0, totalElements: 42 },
        20
      )
    ).toEqual({ totalPages: 3, totalElements: 42 });
  });

  it('falls back to the server rows on the page when totals are absent', () => {
    expect(computeRecordTotals({ content: new Array(5).fill({}) }, 20)).toEqual(
      {
        totalPages: 1,
        totalElements: 5,
      }
    );
  });

  it('defaults to one empty page before any data has loaded', () => {
    expect(computeRecordTotals(undefined, 20)).toEqual({
      totalPages: 1,
      totalElements: 0,
    });
  });
});
