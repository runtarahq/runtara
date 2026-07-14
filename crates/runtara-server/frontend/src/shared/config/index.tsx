import {
  BarChart3,
  Database,
  History,
  Link,
  LineChart,
  Workflow,
  Zap,
} from 'lucide-react';
import {
  isEnabled,
  type EntitlementsSnapshot,
  type FeatureKey,
} from '@/shared/entitlements';
import { checkUserGroup } from '@/lib/utils';

export type MenuChild = {
  key: string;
  title: string;
  to: string;
  icon?: React.ReactNode;
  /** When true, `to` is an absolute path outside the SPA (e.g. the management
   *  SPA at /ui/management/), rendered as a plain anchor doing a full browser
   *  navigation rather than a react-router <Link>. */
  external?: boolean;
};

export type MenuItem = {
  key: string;
  title: string;
  to: string;
  icon: React.ReactNode;
  allowedGroups: string[];
  /** When set, this entry is hidden unless the resolved entitlement snapshot
   *  has the feature enabled. Workflows / Triggers / Connections / Analytics
   *  / Invocation History are intentionally always-on per
   *  `docs/entitlements.md` ("Files / Connections / Triggers / Analytics /
   *  Invocation History" decision). */
  requiresFeature?: FeatureKey;
  children?: MenuChild[];
};

/**
 * Filter menu items by group ACL and entitlement gate. Pure function so the
 * filter logic is testable without rendering the whole Sidebar tree.
 *
 * Order: group ACL first (cheaper and matches the pre-entitlement behavior),
 * then entitlement check. An entry without `requiresFeature` always passes
 * the entitlement gate.
 */
export function filterMenu(
  items: readonly MenuItem[],
  userGroups: string[],
  entitlements: EntitlementsSnapshot
): MenuItem[] {
  return items.filter((item) => {
    if (!checkUserGroup(item.allowedGroups, userGroups)) return false;
    if (item.requiresFeature && !isEnabled(entitlements, item.requiresFeature))
      return false;
    return true;
  });
}

export const menu: MenuItem[] = [
  {
    key: 'workflows',
    title: 'Workflows',
    to: '/workflows',
    icon: <Workflow size={16} />,
    allowedGroups: [],
  },
  {
    key: 'invocation-history',
    title: 'Invocation History',
    to: '/invocation-history',
    icon: <History size={16} />,
    allowedGroups: [],
  },
  {
    key: 'objects',
    title: 'Database',
    to: '/objects/types',
    icon: <Database size={16} />,
    allowedGroups: [],
    requiresFeature: 'database',
  },
  {
    key: 'reports',
    title: 'Reports',
    to: '/reports',
    icon: <LineChart size={16} />,
    allowedGroups: [],
    requiresFeature: 'reports',
  },
  {
    key: 'triggers',
    title: 'Triggers',
    to: '/invocation-triggers',
    icon: <Zap size={16} />,
    allowedGroups: [],
  },
  {
    key: 'connections',
    title: 'Connections',
    to: '/connections',
    icon: <Link size={16} />,
    allowedGroups: [],
  },
  {
    key: 'analytics',
    title: 'Analytics',
    to: '/analytics/usage',
    icon: <BarChart3 size={16} />,
    allowedGroups: [],
    children: [
      { key: 'usage', title: 'Usage', to: '/analytics/usage' },
      { key: 'system', title: 'System', to: '/analytics/system' },
      {
        key: 'rate-limits',
        title: 'Rate Limits',
        to: '/analytics/rate-limits',
      },
    ],
  },
];
