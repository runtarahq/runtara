import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  isWithinMount,
  loadStaticConfig,
  normalizeMountPath,
  parseStaticConfig,
} from './staticConfig';

describe('normalizeMountPath', () => {
  it('defaults to the server mount', () => {
    expect(normalizeMountPath(undefined)).toBe('/ui/');
    expect(normalizeMountPath('  ')).toBe('/ui/');
  });

  it('adds the slashes a path is missing', () => {
    expect(normalizeMountPath('console')).toBe('/console/');
    expect(normalizeMountPath('/a//b')).toBe('/a/b/');
    expect(normalizeMountPath('/')).toBe('/');
  });
});

describe('isWithinMount', () => {
  it('takes the mount with or without its trailing slash', () => {
    expect(isWithinMount('/ui', '/ui/')).toBe(true);
    expect(isWithinMount('/ui/acme/workflows', '/ui/')).toBe(true);
    expect(isWithinMount('/', '/ui/')).toBe(false);
    expect(isWithinMount('/uix', '/ui/')).toBe(false);
  });
});

describe('parseStaticConfig', () => {
  it('marks the config static and keeps the deployment settings', () => {
    expect(
      parseStaticConfig({ authMode: 'oidc', apiBaseUrl: 'https://api.test' })
    ).toEqual({
      authMode: 'oidc',
      apiBaseUrl: 'https://api.test',
      hosting: 'static',
    });
  });

  it('drops what only a build or a server knows', () => {
    expect(
      parseStaticConfig({
        version: '1.0.0',
        commit: 'abc',
        buildNumber: '7',
        entitlements: {},
        mountPath: '/',
      })
    ).toEqual({ mountPath: '/', hosting: 'static' });
  });

  it('rejects anything but an object', () => {
    expect(parseStaticConfig(null)).toBeUndefined();
    expect(parseStaticConfig([])).toBeUndefined();
    expect(parseStaticConfig('oidc')).toBeUndefined();
  });
});

describe('loadStaticConfig', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    window.__RUNTARA_CONFIG__ = undefined;
  });

  const respond = (body: string, contentType: string) =>
    vi.stubGlobal(
      'fetch',
      vi.fn(
        async () =>
          new Response(body, { headers: { 'content-type': contentType } })
      )
    );

  it('leaves a server-injected config alone', async () => {
    const fetch = vi.fn();
    vi.stubGlobal('fetch', fetch);
    window.__RUNTARA_CONFIG__ = { authMode: 'local' };

    expect(await loadStaticConfig()).toBe(true);
    expect(fetch).not.toHaveBeenCalled();
    expect(window.__RUNTARA_CONFIG__).toEqual({ authMode: 'local' });
  });

  it('ignores an SPA fallback answering for a missing config.json', async () => {
    respond('<!doctype html>', 'text/html');
    window.__RUNTARA_CONFIG__ = {};

    expect(await loadStaticConfig()).toBe(true);
    expect(window.__RUNTARA_CONFIG__).toEqual({});
  });

  it('sends a page outside the mount to the mount', async () => {
    respond(JSON.stringify({ authMode: 'local' }), 'application/json');
    const replace = vi.fn();
    vi.stubGlobal('location', { pathname: '/', replace });

    expect(await loadStaticConfig()).toBe(false);
    expect(replace).toHaveBeenCalledWith('/ui/');
  });

  it('installs config.json when the page is under the mount', async () => {
    respond(
      JSON.stringify({ authMode: 'local', mountPath: '/' }),
      'application/json'
    );

    expect(await loadStaticConfig()).toBe(true);
    expect(window.__RUNTARA_CONFIG__).toEqual({
      authMode: 'local',
      mountPath: '/',
      hosting: 'static',
    });
  });
});
