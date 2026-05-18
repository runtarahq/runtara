import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { normalizePlausibleHost } from './plausible';

const originalPushState = history.pushState;
const originalReplaceState = history.replaceState;

const runtimeWindow = window as typeof window & {
  __RUNTARA_CONFIG__?: Record<string, string>;
};

describe('normalizePlausibleHost', () => {
  it('defaults to plausible.io when no host is configured', () => {
    expect(normalizePlausibleHost(undefined)).toBe('https://plausible.io');
  });

  it('treats scheme-less hosts as HTTPS origins', () => {
    expect(normalizePlausibleHost('metrics.syncmyorders.com')).toBe(
      'https://metrics.syncmyorders.com'
    );
  });

  it('preserves explicit and proxied hosts', () => {
    expect(normalizePlausibleHost('http://localhost:8000/')).toBe(
      'http://localhost:8000'
    );
    expect(normalizePlausibleHost('//metrics.syncmyorders.com/')).toBe(
      'https://metrics.syncmyorders.com'
    );
    expect(normalizePlausibleHost('/plausible/')).toBe('/plausible');
  });
});

describe('initAnalytics', () => {
  beforeEach(() => {
    vi.resetModules();
    document.head.innerHTML = '';
    delete window.plausible;
    delete runtimeWindow.__RUNTARA_CONFIG__;
    history.pushState = originalPushState;
    history.replaceState = originalReplaceState;
  });

  afterEach(() => {
    document.head.innerHTML = '';
    delete window.plausible;
    delete runtimeWindow.__RUNTARA_CONFIG__;
    history.pushState = originalPushState;
    history.replaceState = originalReplaceState;
  });

  it('injects an absolute script URL for a scheme-less Plausible host', async () => {
    runtimeWindow.__RUNTARA_CONFIG__ = {
      plausibleDomain: 'app.syncmyorders.com',
      plausibleHost: 'metrics.syncmyorders.com',
    };

    const { initAnalytics } = await import('./plausible');

    initAnalytics();

    const script = document.head.querySelector<HTMLScriptElement>(
      'script[data-domain="app.syncmyorders.com"]'
    );

    expect(script?.src).toBe(
      'https://metrics.syncmyorders.com/js/script.manual.js'
    );
  });
});
