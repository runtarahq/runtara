import {
  AlertCircle,
  AlertTriangle,
  History,
  CheckCircle2,
  Settings,
  Layers,
} from 'lucide-react';
import { Badge } from '@/shared/components/ui/badge';
import {
  useValidationStore,
  BottomPanelTab,
} from '../../stores/validationStore';
import { cn } from '@/lib/utils';

interface BottomPanelTabsProps {
  versionCount?: number;
}

/**
 * Tab switcher for the bottom panel.
 * Allows switching between Problems, History, Settings, and Versions tabs.
 */
export function BottomPanelTabs({ versionCount = 0 }: BottomPanelTabsProps) {
  const activeTab = useValidationStore((s) => s.activeTab);
  const setActiveTab = useValidationStore((s) => s.setActiveTab);
  const errorCount = useValidationStore((s) => s.getErrorCount());
  const warningCount = useValidationStore((s) => s.getWarningCount());

  const hasProblems = errorCount > 0 || warningCount > 0;

  const tabs: {
    id: BottomPanelTab;
    label: string;
    icon: React.ReactNode;
    badge?: React.ReactNode;
  }[] = [
    {
      id: 'versions',
      label: 'Versions',
      icon: <Layers className="h-4 w-4" />,
      badge:
        versionCount > 0 ? (
          <Badge variant="secondary" className="h-5 px-1.5">
            {versionCount}
          </Badge>
        ) : null,
    },
    {
      id: 'settings',
      label: 'Settings',
      icon: <Settings className="h-4 w-4" />,
    },
    {
      id: 'problems',
      label: 'Problems',
      icon: hasProblems ? (
        <AlertCircle className="h-4 w-4" />
      ) : (
        <CheckCircle2 className="h-4 w-4 text-success" />
      ),
      badge: hasProblems ? (
        <span className="flex items-center gap-1">
          {errorCount > 0 && (
            <Badge variant="destructive" className="h-5 px-1.5 gap-0.5">
              <AlertCircle className="h-3 w-3" />
              {errorCount}
            </Badge>
          )}
          {warningCount > 0 && (
            <Badge variant="warning" className="h-5 px-1.5 gap-0.5">
              <AlertTriangle className="h-3 w-3" />
              {warningCount}
            </Badge>
          )}
        </span>
      ) : (
        <Badge variant="success" className="h-5">
          OK
        </Badge>
      ),
    },
    {
      id: 'history',
      label: 'History',
      icon: <History className="h-4 w-4" />,
    },
  ];

  return (
    <div className="flex items-center gap-1 px-2">
      {tabs.map((tab) => (
        <button
          key={tab.id}
          type="button"
          onClick={() => setActiveTab(tab.id)}
          className={cn(
            'flex items-center gap-2 px-3 py-1.5 text-sm font-medium rounded-t-md transition-colors border-b-2',
            activeTab === tab.id
              ? 'bg-card text-foreground border-primary'
              : 'text-muted-foreground hover:text-foreground border-transparent hover:bg-muted/50'
          )}
        >
          {tab.icon}
          <span>{tab.label}</span>
          {tab.badge}
        </button>
      ))}
    </div>
  );
}
