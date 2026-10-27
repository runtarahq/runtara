import { describe, expect, it } from 'vitest';
import { chooseTenant, tenantHome } from './myTenants';

describe('chooseTenant', () => {
  it('goes straight to the only tenant', () => {
    expect(chooseTenant([{ id: 'acme', role: 'owner' }])).toEqual({
      kind: 'redirect',
      id: 'acme',
    });
  });

  it('lets the user pick among several', () => {
    const tenants = [
      { id: 'acme', role: 'owner' },
      { id: 'beta', role: 'viewer' },
    ];
    expect(chooseTenant(tenants)).toEqual({ kind: 'pick', tenants });
  });

  it('reports no tenant when the user is a member of none', () => {
    expect(chooseTenant([])).toEqual({ kind: 'none' });
  });
});

describe('tenantHome', () => {
  it('appends the tenant to the mount', () => {
    expect(tenantHome('/ui/', 'acme')).toBe('/ui/acme/');
    expect(tenantHome('/ui', 'acme')).toBe('/ui/acme/');
  });
});
