import { describe, expect, it } from 'vitest';
import { filterMenu, menu } from './index';
import { PERMISSIVE_FALLBACK } from '@/shared/entitlements';
import type { EntitlementsSnapshot } from '@/shared/entitlements';

function snapshot(
  overrides: Partial<EntitlementsSnapshot['features']> = {}
): EntitlementsSnapshot {
  return {
    ...PERMISSIVE_FALLBACK,
    features: { ...PERMISSIVE_FALLBACK.features, ...overrides },
  };
}

const keys = (items: ReturnType<typeof filterMenu>) =>
  items.map((item) => item.key);

describe('filterMenu — entitlement gate', () => {
  it('returns every menu entry when all features are enabled', () => {
    const out = filterMenu(menu, [], snapshot());
    expect(keys(out)).toEqual([
      'workflows',
      'invocation-history',
      'objects',
      'reports',
      'triggers',
      'connections',
      'analytics',
    ]);
  });

  it('hides Reports when reports is disabled', () => {
    const out = filterMenu(menu, [], snapshot({ reports: false }));
    expect(keys(out)).not.toContain('reports');
    // Other entries unaffected.
    expect(keys(out)).toContain('workflows');
    expect(keys(out)).toContain('objects');
  });

  it('hides Database when database is disabled', () => {
    const out = filterMenu(menu, [], snapshot({ database: false }));
    expect(keys(out)).not.toContain('objects');
    expect(keys(out)).toContain('workflows');
    expect(keys(out)).toContain('reports');
  });

  it('hides both Reports and Database when both are disabled', () => {
    const out = filterMenu(
      menu,
      [],
      snapshot({ reports: false, database: false })
    );
    expect(keys(out)).not.toContain('reports');
    expect(keys(out)).not.toContain('objects');
  });

  it('always-on entries stay visible regardless of feature flags', () => {
    // Disabling reports + database must not hide workflows / triggers /
    // connections / analytics / invocation-history — those are tier-independent
    // per docs/entitlements.md.
    const out = filterMenu(
      menu,
      [],
      snapshot({ reports: false, database: false, api: false, mcp: false })
    );
    expect(keys(out)).toEqual([
      'workflows',
      'invocation-history',
      'triggers',
      'connections',
      'analytics',
    ]);
  });
});

describe('filterMenu — group ACL preserved', () => {
  it('drops entries the user is not allowed to see, regardless of entitlements', () => {
    const restricted = [
      ...menu,
      {
        key: 'admin',
        title: 'Admin',
        to: '/admin',
        icon: null as unknown as React.ReactNode,
        allowedGroups: ['platform-admins'],
      },
    ];
    const out = filterMenu(restricted, ['regular-user'], snapshot());
    expect(keys(out)).not.toContain('admin');
  });

  it('keeps gated entries the user is allowed to see only when the feature is on', () => {
    const out = filterMenu(menu, [], snapshot({ reports: false }));
    expect(keys(out)).not.toContain('reports');

    const out2 = filterMenu(menu, [], snapshot({ reports: true }));
    expect(keys(out2)).toContain('reports');
  });
});
