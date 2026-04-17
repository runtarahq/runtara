import { ChevronDown, ChevronUp, X } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { useValidationStore } from '../../stores/validationStore';
import { BottomPanelTabs } from './BottomPanelTabs';

interface ValidationPanelHeaderProps {
  versionCount?: number;
}

/**
 * Header for the bottom panel.
 * Contains tabs for switching between Problems, History, Settings, and Versions,
 * and collapse/expand toggle.
 */
export function ValidationPanelHeader({
  versionCount = 0,
}: ValidationPanelHeaderProps) {
  const isPanelExpanded = useValidationStore((s) => s.isPanelExpanded);
  const togglePanel = useValidationStore((s) => s.togglePanel);
  const clearMessages = useValidationStore((s) => s.clearMessages);
  const activeTab = useValidationStore((s) => s.activeTab);
  const errorCount = useValidationStore((s) => s.getErrorCount());
  const warningCount = useValidationStore((s) => s.getWarningCount());

  const hasProblems = errorCount > 0 || warningCount > 0;
  const showClearButton = activeTab === 'problems' && hasProblems;

  return (
    <div className="flex h-10 shrink-0 items-center justify-between border-b bg-muted/30">
      <div className="flex items-center">
        <button
          type="button"
          onClick={togglePanel}
          className="flex items-center justify-center w-10 h-10 text-muted-foreground hover:text-foreground transition-colors"
          title={isPanelExpanded ? 'Collapse panel' : 'Expand panel'}
        >
          {isPanelExpanded ? (
            <ChevronDown className="h-4 w-4" />
          ) : (
            <ChevronUp className="h-4 w-4" />
          )}
        </button>
        <BottomPanelTabs versionCount={versionCount} />
      </div>

      {showClearButton && (
        <Button
          variant="ghost"
          size="icon"
          className="h-7 w-7 mr-2"
          onClick={(e) => {
            e.stopPropagation();
            clearMessages();
          }}
          title="Clear all messages"
        >
          <X className="h-4 w-4" />
        </Button>
      )}
    </div>
  );
}
