/**
 * The workflow listing's view state, carried in the URL query string.
 *
 * The folder was already a query parameter, so reloading a folder kept it while
 * the search, the page and the page size — plain component state — were thrown
 * away. Putting all of them in the URL makes the whole view reloadable,
 * navigable with back/forward, and shareable as a link.
 *
 * Defaults are elided rather than written, so the default view has one
 * canonical URL instead of `?page=1&pageSize=10`. The values are user-editable
 * text, so reading them validates rather than trusts.
 */

import { PAGE_SIZE_OPTIONS } from '@/shared/components/console';

export const DEFAULT_PAGE_SIZE = 10;

export const PAGE_PARAM = 'page';
export const PAGE_SIZE_PARAM = 'pageSize';
export const SEARCH_PARAM = 'q';

export interface ListUrlState {
  /** 0-based, as the listing API and the pagination control expect. */
  page: number;
  pageSize: number;
  search: string;
}

/**
 * Read the listing state out of the query string.
 *
 * `page` is 1-based in the URL — it should match the "Page 2 of 11" the footer
 * shows — and 0-based everywhere else.
 */
export function readListUrlState(params: URLSearchParams): ListUrlState {
  const rawPage = Number(params.get(PAGE_PARAM));
  const page = Number.isInteger(rawPage) && rawPage > 1 ? rawPage - 1 : 0;

  const rawPageSize = Number(params.get(PAGE_SIZE_PARAM));
  const pageSize = PAGE_SIZE_OPTIONS.includes(rawPageSize)
    ? rawPageSize
    : DEFAULT_PAGE_SIZE;

  return { page, pageSize, search: params.get(SEARCH_PARAM) ?? '' };
}

/**
 * Apply a change to the listing state, returning the new query string.
 *
 * Changing the search or the page size resizes or refills the listing, so the
 * page it was showing no longer means anything: both drop it. Doing that here
 * rather than in an effect afterwards keeps one URL update — and so one history
 * entry — per user action.
 */
export function writeListUrlState(
  params: URLSearchParams,
  patch: Partial<ListUrlState>
): URLSearchParams {
  const next = new URLSearchParams(params);

  if (patch.search !== undefined) {
    if (patch.search.trim()) next.set(SEARCH_PARAM, patch.search);
    else next.delete(SEARCH_PARAM);
    next.delete(PAGE_PARAM);
  }

  if (patch.pageSize !== undefined) {
    if (patch.pageSize === DEFAULT_PAGE_SIZE) next.delete(PAGE_SIZE_PARAM);
    else next.set(PAGE_SIZE_PARAM, String(patch.pageSize));
    next.delete(PAGE_PARAM);
  }

  if (patch.page !== undefined) {
    if (patch.page > 0) next.set(PAGE_PARAM, String(patch.page + 1));
    else next.delete(PAGE_PARAM);
  }

  return next;
}
