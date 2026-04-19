import { useValidationStore } from '../../stores/validationStore';
import { ValidationPanelHeader } from './ValidationPanelHeader';
import { ValidationPanelContent } from './ValidationPanelContent';
import { HistoryPanelContent } from './HistoryPanelContent';
import { SettingsContent } from './SettingsContent';
import { VersionsPanelContent } from './VersionsPanelContent';
import { WorkflowData } from '../WorkflowEditor/EditorSidebar';
import { WorkflowVersionInfoDto } from '@/features/workflows/queries';
import { cn } from '@/lib/utils';

interface ValidationPanelProps {
  /** Callback when user clicks to navigate to a step */
  onNavigateToStep: (stepId: string) => void;
  /** The current workflow ID for history tab */
  workflowId: string;
  /** Workflow data for settings tabs */
  workflow: WorkflowData;
  /** Callback when workflow settings change */
  onWorkflowChange: (data: Partial<WorkflowData>) => void;
  /** Whether the panel is in read-only mode */
  readOnly?: boolean;
  /** Available versions for the versions tab */
  versions?: WorkflowVersionInfoDto[];
  /** Currently selected/viewing version */
  selectedVersion?: number;
  /** Currently active (deployed) version */
  currentVersionNumber?: number;
  /** Callback when user selects a different version to view */
  onVersionChange?: (version: number | undefined) => void;
  /** Callback when user activates a version */
  onVersionActivate?: (version: number) => void;
  /** Whether version operations are loading */
  isVersionLoading?: boolean;
}

/**
 * Bottom-docked panel with tabs for Problems, History, Settings, and Versions.
 * Problems tab shows validation errors and warnings.
 * History tab shows recent execution instances.
 * Settings tab shows workflow configuration with split view.
 * Versions tab shows all workflow versions with controls.
 * Always visible - shows "No problems" when there are no issues.
 */
export function ValidationPanel({
  onNavigateToStep,
  workflowId,
  workflow,
  onWorkflowChange,
  readOnly = false,
  versions = [],
  selectedVersion,
  currentVersionNumber,
  onVersionChange,
  onVersionActivate,
  isVersionLoading = false,
}: ValidationPanelProps) {
  const isPanelExpanded = useValidationStore((s) => s.isPanelExpanded);
  const activeTab = useValidationStore((s) => s.activeTab);

  return (
    <div
      className={cn(
        'border-t bg-card transition-all duration-200 flex flex-col',
        isPanelExpanded ? 'h-[320px]' : 'h-10'
      )}
    >
      <ValidationPanelHeader versionCount={versions.length} />
      {isPanelExpanded && (
        <>
          {activeTab === 'problems' && (
            <ValidationPanelContent onNavigateToStep={onNavigateToStep} />
          )}
          {activeTab === 'history' && (
            <HistoryPanelContent workflowId={workflowId} />
          )}
          {activeTab === 'settings' && (
            <SettingsContent
              workflow={workflow}
              onChange={onWorkflowChange}
              readOnly={readOnly}
            />
          )}
          {activeTab === 'versions' && (
            <VersionsPanelContent
              versions={versions}
              selectedVersion={selectedVersion}
              currentVersionNumber={currentVersionNumber}
              onVersionChange={onVersionChange ?? (() => {})}
              onVersionActivate={onVersionActivate ?? (() => {})}
              isLoading={isVersionLoading}
            />
          )}
        </>
      )}
    </div>
  );
}
