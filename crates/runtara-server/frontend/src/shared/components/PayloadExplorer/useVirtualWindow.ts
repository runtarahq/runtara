import {
  useCallback,
  useLayoutEffect,
  useRef,
  useState,
  type UIEvent,
} from 'react';

export interface VirtualWindow {
  /** First rendered index (inclusive). */
  start: number;
  /** Last rendered index (exclusive). */
  end: number;
  /** Height of the full list, for the spacer that sizes the scrollbar. */
  totalHeight: number;
  /** Offset of the first rendered row from the top of the list. */
  offsetTop: number;
}

/**
 * Which rows to render for a fixed-height list scrolled to `scrollTop`.
 * `scrollTop` is clamped to the list, so a list that shrank (a branch folded)
 * never renders an empty window.
 */
export function computeWindow(
  count: number,
  rowHeight: number,
  viewportHeight: number,
  scrollTop: number,
  overscan: number
): VirtualWindow {
  const totalHeight = count * rowHeight;
  const maxScroll = Math.max(0, totalHeight - viewportHeight);
  const top = Math.min(Math.max(0, scrollTop), maxScroll);
  const start = Math.max(0, Math.floor(top / rowHeight) - overscan);
  const end = Math.min(
    count,
    Math.ceil((top + viewportHeight) / rowHeight) + overscan
  );
  return { start, end, totalHeight, offsetTop: start * rowHeight };
}

/**
 * Scroll position that brings row `index` fully into view, moving as little
 * as possible; returns `scrollTop` unchanged when it already is.
 */
export function scrollTopForIndex(
  index: number,
  rowHeight: number,
  viewportHeight: number,
  scrollTop: number
): number {
  const rowTop = index * rowHeight;
  const rowBottom = rowTop + rowHeight;
  if (rowTop < scrollTop) return rowTop;
  if (rowBottom > scrollTop + viewportHeight) return rowBottom - viewportHeight;
  return scrollTop;
}

/**
 * Windowing for a fixed-row-height list. Heights come from props rather than
 * layout measurement, so rendering is identical with or without layout (jsdom,
 * hidden tabs) and needs no ResizeObserver.
 */
export function useVirtualWindow({
  count,
  rowHeight,
  viewportHeight,
  overscan = 8,
}: {
  count: number;
  rowHeight: number;
  viewportHeight: number;
  overscan?: number;
}) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const pendingIndex = useRef<number | null>(null);

  const onScroll = useCallback((event: UIEvent<HTMLDivElement>) => {
    setScrollTop(event.currentTarget.scrollTop);
  }, []);

  const scrollToIndex = useCallback(
    (index: number) => {
      pendingIndex.current = index;
      const el = scrollRef.current;
      if (!el) return;
      const next = scrollTopForIndex(
        index,
        rowHeight,
        viewportHeight,
        el.scrollTop
      );
      if (next !== el.scrollTop) el.scrollTop = next;
      // The browser clamps scrollTop to the current content height; keep the
      // request pending only when it could not be applied yet.
      if (Math.abs(el.scrollTop - next) < 1) pendingIndex.current = null;
      setScrollTop(el.scrollTop);
    },
    [rowHeight, viewportHeight]
  );

  // A scroll requested in the same update that changed the row count (expand
  // to reveal a match, then scroll to it) is re-applied once the taller spacer
  // has rendered and the browser accepts the new scrollTop.
  useLayoutEffect(() => {
    const index = pendingIndex.current;
    const el = scrollRef.current;
    if (index === null || !el) return;
    pendingIndex.current = null;
    const next = scrollTopForIndex(
      index,
      rowHeight,
      viewportHeight,
      el.scrollTop
    );
    if (next !== el.scrollTop) {
      el.scrollTop = next;
      setScrollTop(next);
    }
  }, [count, rowHeight, viewportHeight]);

  return {
    scrollRef,
    onScroll,
    scrollToIndex,
    ...computeWindow(count, rowHeight, viewportHeight, scrollTop, overscan),
  };
}
