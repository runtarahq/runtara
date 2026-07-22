import { useLayoutEffect, useRef, useState } from 'react';

/**
 * Observe an element's content-box width. Returns `null` while the width is
 * unknown — before the first observation, when the element is hidden
 * (0-width), or where ResizeObserver doesn't exist (jsdom) — so callers can
 * fall back to unconstrained layout instead of treating "unknown" as "0".
 */
export function useContainerWidth<T extends HTMLElement>() {
  const ref = useRef<T | null>(null);
  const [width, setWidth] = useState<number | null>(null);

  useLayoutEffect(() => {
    const element = ref.current;
    if (!element || typeof ResizeObserver === 'undefined') return;

    const observer = new ResizeObserver((entries) => {
      const entry = entries[entries.length - 1];
      const next = Math.round(entry.contentRect.width);
      setWidth(next > 0 ? next : null);
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  return [ref, width] as const;
}
