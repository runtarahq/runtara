import { describe, expect, it } from 'vitest';

import { buildFolderCounts, recursiveWorkflowCount } from './folder-counts';

/** Mirrors the tenant the deep-test ran against: 124 workflows, 5 folders. */
const COUNTS = [
  { path: '/', workflowCount: 110 },
  { path: '/Commerce/', workflowCount: 2 },
  { path: '/Customer/', workflowCount: 2 },
  { path: '/Demo/', workflowCount: 1 },
  { path: '/Demo/Test/', workflowCount: 5 },
  { path: '/Microsoft Azure/', workflowCount: 2 },
  { path: '/Operations/', workflowCount: 2 },
];

describe('recursiveWorkflowCount', () => {
  it('includes workflows held in subfolders', () => {
    // /Demo/ holds 1 directly and 5 in /Demo/Test/. Counting direct children
    // only is what made this read "1 workflow".
    expect(recursiveWorkflowCount(COUNTS, '/Demo/')).toBe(6);
  });

  it('counts a leaf folder as itself', () => {
    expect(recursiveWorkflowCount(COUNTS, '/Demo/Test/')).toBe(5);
    expect(recursiveWorkflowCount(COUNTS, '/Commerce/')).toBe(2);
  });

  it('counts the whole tenant at the root', () => {
    expect(recursiveWorkflowCount(COUNTS, '/')).toBe(124);
  });

  it('counts a folder the server never reported', () => {
    // parseFolderPaths synthesizes intermediate folders. "/Parent/" holds no
    // workflows of its own, so the server omits it — it must still total 3.
    const nested = [
      { path: '/Parent/Child/', workflowCount: 3 },
      { path: '/Elsewhere/', workflowCount: 9 },
    ];
    expect(recursiveWorkflowCount(nested, '/Parent/')).toBe(3);
  });

  it('does not let a sibling with a shared prefix leak in', () => {
    // The trailing slash is what keeps "/DemoArchive/" out of "/Demo/".
    const siblings = [
      { path: '/Demo/', workflowCount: 1 },
      { path: '/DemoArchive/', workflowCount: 50 },
    ];
    expect(recursiveWorkflowCount(siblings, '/Demo/')).toBe(1);
  });

  it('reports zero for a folder with nothing under it', () => {
    expect(recursiveWorkflowCount(COUNTS, '/Empty/')).toBe(0);
  });
});

describe('buildFolderCounts', () => {
  it('resolves every requested folder', () => {
    const result = buildFolderCounts(COUNTS, [
      '/Commerce/',
      '/Customer/',
      '/Demo/',
      '/Microsoft Azure/',
      '/Operations/',
    ]);

    expect(result).toEqual({
      '/Commerce/': 2,
      '/Customer/': 2,
      '/Demo/': 6,
      '/Microsoft Azure/': 2,
      '/Operations/': 2,
    });
  });

  it('returns nothing until counts have loaded', () => {
    // Empty, not zeros — the caller renders blank rather than a wrong "0".
    expect(buildFolderCounts(undefined, ['/Commerce/'])).toEqual({});
  });

  it('distinguishes a loaded zero from an unknown count', () => {
    expect(buildFolderCounts([], ['/Commerce/'])).toEqual({ '/Commerce/': 0 });
  });
});
