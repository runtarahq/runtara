import { useCallback, useRef, useState } from 'react';

/**
 * Observe an element's content-box width. Returns `null` while the width is
 * unknown — before the first observation, when the element is hidden
 * (0-width), or where ResizeObserver doesn't exist (jsdom) — so callers can
 * fall back to unconstrained layout instead of treating "unknown" as "0".
 *
 * The first element is a callback ref: attaching/detaching the observer
 * follows the node through conditional renders (loading and empty branches
 * mount without the observed node; a mount-time effect would miss the node
 * appearing later).
 */
export function useContainerWidth<T extends HTMLElement>() {
  const [width, setWidth] = useState<number | null>(null);
  const observerRef = useRef<ResizeObserver | null>(null);

  const ref = useCallback((node: T | null) => {
    observerRef.current?.disconnect();
    observerRef.current = null;

    if (!node || typeof ResizeObserver === 'undefined') {
      setWidth(null);
      return;
    }

    const observer = new ResizeObserver((entries) => {
      const entry = entries[entries.length - 1];
      const next = Math.round(entry.contentRect.width);
      setWidth(next > 0 ? next : null);
    });
    observer.observe(node);
    observerRef.current = observer;
  }, []);

  return [ref, width] as const;
}
