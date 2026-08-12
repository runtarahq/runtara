/**
 * Row counts a console table offers in its pagination control.
 *
 * Its own module so callers that only need the values — validating a `pageSize`
 * read out of a URL, say — don't import a component to get them.
 */
export const PAGE_SIZE_OPTIONS = [10, 20, 50, 100];
