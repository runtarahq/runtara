import { describe, expect, it } from 'vitest';
import {
  createWorkflowHref,
  normalizeFolderParam,
  workflowsListHref,
} from './folder-nav';

describe('createWorkflowHref', () => {
  it('omits the folder param at the root', () => {
    expect(createWorkflowHref('/')).toBe('/workflows/create');
  });

  it('carries the current folder as an encoded search param', () => {
    expect(createWorkflowHref('/Demo/Test/')).toBe(
      '/workflows/create?folder=%2FDemo%2FTest%2F'
    );
  });
});

describe('workflowsListHref', () => {
  it('returns the bare list route for the root', () => {
    expect(workflowsListHref('/')).toBe('/workflows');
  });

  it('restores the folder the user came from', () => {
    expect(workflowsListHref('/Demo/')).toBe('/workflows?folder=%2FDemo%2F');
  });
});

describe('normalizeFolderParam', () => {
  it('falls back to root for a missing param', () => {
    expect(normalizeFolderParam(null)).toBe('/');
    expect(normalizeFolderParam('')).toBe('/');
  });

  it('accepts the root itself', () => {
    expect(normalizeFolderParam('/')).toBe('/');
  });

  it('accepts a /-wrapped folder path', () => {
    expect(normalizeFolderParam('/Demo/Test/')).toBe('/Demo/Test/');
  });

  it('rejects paths missing a leading or trailing slash', () => {
    expect(normalizeFolderParam('Demo/')).toBe('/');
    expect(normalizeFolderParam('/Demo')).toBe('/');
  });

  it('rejects paths with empty segments', () => {
    expect(normalizeFolderParam('//')).toBe('/');
    expect(normalizeFolderParam('/Demo//Test/')).toBe('/');
  });
});
