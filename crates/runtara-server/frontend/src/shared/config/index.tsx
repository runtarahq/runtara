import {
  BarChart3,
  Database,
  FolderOpen,
  History,
  Link,
  Workflow,
  Zap,
} from 'lucide-react';

export const menu = [
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
  },
  {
    key: 'files',
    title: 'Files',
    to: '/files',
    icon: <FolderOpen size={16} />,
    allowedGroups: [],
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
