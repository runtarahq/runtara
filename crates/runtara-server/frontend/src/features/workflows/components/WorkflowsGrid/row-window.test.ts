import { describe, expect, it } from 'vitest';

import {
  folderWorkflowWindow,
  rowPageCount,
  workflowServerSlice,
} from './row-window';

describe('folderWorkflowWindow', () => {
  it('fills the page with workflows when there are no folders', () => {
    expect(folderWorkflowWindow(0, 0, 10)).toEqual({
      folderStart: 0,
      folderTake: 0,
      workflowOffset: 0,
      workflowLimit: 10,
    });
    expect(folderWorkflowWindow(0, 3, 10)).toEqual({
      folderStart: 0,
      folderTake: 0,
      workflowOffset: 30,
      workflowLimit: 10,
    });
  });

  it('puts folders first and fills the rest of the page with workflows', () => {
    expect(folderWorkflowWindow(3, 0, 10)).toEqual({
      folderStart: 0,
      folderTake: 3,
      workflowOffset: 0,
      workflowLimit: 7,
    });
  });

  it('never repeats folders on later pages', () => {
    // The bug: page 2 used to render all 3 folders again above its workflows.
    expect(folderWorkflowWindow(3, 1, 10)).toEqual({
      folderStart: 3,
      folderTake: 0,
      workflowOffset: 7,
      workflowLimit: 10,
    });
  });

  it('spreads a folder list longer than a page across pages', () => {
    expect(folderWorkflowWindow(12, 0, 10)).toEqual({
      folderStart: 0,
      folderTake: 10,
      workflowOffset: 0,
      workflowLimit: 0,
    });
    // Boundary page: the last folders, then workflows from the very beginning.
    expect(folderWorkflowWindow(12, 1, 10)).toEqual({
      folderStart: 10,
      folderTake: 2,
      workflowOffset: 0,
      workflowLimit: 8,
    });
    expect(folderWorkflowWindow(12, 2, 10)).toEqual({
      folderStart: 12,
      folderTake: 0,
      workflowOffset: 8,
      workflowLimit: 10,
    });
  });

  it('starts workflows on a clean page when folders fill an exact number of pages', () => {
    expect(folderWorkflowWindow(20, 2, 10)).toEqual({
      folderStart: 20,
      folderTake: 0,
      workflowOffset: 0,
      workflowLimit: 10,
    });
  });
});

describe('workflowServerSlice', () => {
  it('maps an aligned window straight onto a server page', () => {
    expect(workflowServerSlice(0, 10, 10)).toEqual({
      page: 0,
      skip: 0,
      take: 10,
    });
    expect(workflowServerSlice(30, 10, 10)).toEqual({
      page: 3,
      skip: 0,
      take: 10,
    });
  });

  it('reports a window that straddles two server pages', () => {
    const slice = workflowServerSlice(7, 10, 10);

    expect(slice).toEqual({ page: 0, skip: 7, take: 10 });
    // skip + take past pageSize is the caller's cue to fetch the next page too.
    expect(slice.skip + slice.take).toBeGreaterThan(10);
  });

  it('handles a partial window at the folder boundary', () => {
    expect(workflowServerSlice(0, 8, 10)).toEqual({
      page: 0,
      skip: 0,
      take: 8,
    });
  });

  it('takes nothing for a page made entirely of folders', () => {
    expect(workflowServerSlice(0, 0, 10)).toEqual({
      page: 0,
      skip: 0,
      take: 0,
    });
  });
});

describe('rowPageCount', () => {
  it('counts folders and workflows as one row set', () => {
    expect(rowPageCount(12, 43, 10)).toBe(6);
    expect(rowPageCount(0, 43, 10)).toBe(5);
    expect(rowPageCount(3, 7, 10)).toBe(1);
    expect(rowPageCount(3, 8, 10)).toBe(2);
  });

  it('always reports at least one page', () => {
    expect(rowPageCount(0, 0, 10)).toBe(1);
  });
});
