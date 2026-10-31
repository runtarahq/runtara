import { describe, expect, it } from 'vitest';
import { resolveStaticBase } from './appBase';

describe('resolveStaticBase', () => {
  const open = ['/invitations/:id'];

  it('roots the SPA at the tenant the first segment names', () => {
    expect(resolveStaticBase('/ui/acme/workflows/1', '/ui/', open)).toEqual({
      path: '/ui/acme/',
      tenant: 'acme',
    });
    expect(resolveStaticBase('/ui/acme', '/ui/', open)).toEqual({
      path: '/ui/acme/',
      tenant: 'acme',
    });
  });

  it('roots the SPA at the mount when no tenant is named', () => {
    expect(resolveStaticBase('/ui/', '/ui/', open)).toEqual({ path: '/ui/' });
    expect(resolveStaticBase('/ui', '/ui/', open)).toEqual({ path: '/ui/' });
    expect(resolveStaticBase('/elsewhere', '/ui/', open)).toEqual({
      path: '/ui/',
    });
  });

  it("serves the edition's open pages at the mount", () => {
    expect(resolveStaticBase('/ui/invitations/x1', '/ui/', open)).toEqual({
      path: '/ui/',
    });
  });

  it('takes tenants straight below a root mount', () => {
    expect(resolveStaticBase('/acme/workflows', '/', open)).toEqual({
      path: '/acme/',
      tenant: 'acme',
    });
    expect(resolveStaticBase('/', '/', open)).toEqual({ path: '/' });
  });

  it('decodes the tenant id', () => {
    expect(resolveStaticBase('/ui/org%20a/', '/ui/', open).tenant).toBe(
      'org a'
    );
  });

  it('keeps a malformed escape as it is', () => {
    expect(resolveStaticBase('/ui/org%E0%A4%A/', '/ui/', open).tenant).toBe(
      'org%E0%A4%A'
    );
  });
});
