type PlausibleFn = ((event: string, options?: { props?: Record<string, unknown> }) => void) & {
  q?: unknown[];
};

declare global {
  interface Window {
    plausible?: PlausibleFn;
  }
}

export function initAnalytics(): void {
  const domain = import.meta.env.VITE_RUNTARA_PLAUSIBLE_DOMAIN?.trim();
  if (!domain) return;

  const host =
    import.meta.env.VITE_RUNTARA_PLAUSIBLE_HOST?.trim() || 'https://plausible.io';

  // Queue early events until the script loads.
  if (!window.plausible) {
    const queued: PlausibleFn = ((...args: unknown[]) => {
      (queued.q = queued.q || []).push(args);
    }) as PlausibleFn;
    window.plausible = queued;
  }

  const script = document.createElement('script');
  script.defer = true;
  script.setAttribute('data-domain', domain);
  // script.manual.js: we fire pageviews ourselves (React Router uses history.pushState,
  // which the default Plausible script doesn't intercept).
  script.src = `${host.replace(/\/$/, '')}/js/script.manual.js`;
  document.head.appendChild(script);

  const firePageview = () => window.plausible?.('pageview');

  firePageview();

  const wrap = (name: 'pushState' | 'replaceState') => {
    const original = history[name];
    history[name] = function (this: History, ...args: Parameters<History[typeof name]>) {
      const result = original.apply(this, args);
      firePageview();
      return result;
    } as History[typeof name];
  };
  wrap('pushState');
  wrap('replaceState');
  window.addEventListener('popstate', firePageview);
}
