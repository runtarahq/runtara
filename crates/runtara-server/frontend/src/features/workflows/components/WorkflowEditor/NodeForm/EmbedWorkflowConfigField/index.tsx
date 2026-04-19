import { useContext, useMemo, useEffect, useRef } from 'react';
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select.tsx';
import { Label } from '@/shared/components/ui/label.tsx';
import { WorkflowDto } from '@/generated/RuntaraRuntimeApi';
import { NodeFormContext } from '../NodeFormContext.tsx';
import { Icons } from '@/shared/components/icons.tsx';
import { cn } from '@/lib/utils.ts';
import { useCustomQuery } from '@/shared/hooks/api';
import { getWorkflowVersions } from '@/features/workflows/queries';
import { queryKeys } from '@/shared/queries/query-keys';

interface EmbedWorkflowConfigFieldProps {
  workflowIdValue: string;
  versionValue: string;
  onWorkflowIdChange: (value: string) => void;
  onVersionChange: (value: string) => void;
  disabled?: boolean;
}

export function EmbedWorkflowConfigField({
  workflowIdValue,
  versionValue,
  onWorkflowIdChange,
  onVersionChange,
  disabled = false,
}: EmbedWorkflowConfigFieldProps) {
  const { workflows } = useContext(NodeFormContext);

  // Use refs to store callbacks to avoid dependency issues
  const onWorkflowIdChangeRef = useRef(onWorkflowIdChange);
  const onVersionChangeRef = useRef(onVersionChange);

  useEffect(() => {
    onWorkflowIdChangeRef.current = onWorkflowIdChange;
    onVersionChangeRef.current = onVersionChange;
  }, [onWorkflowIdChange, onVersionChange]);

  // Use workflow ID directly (no JSON parsing needed)
  const currentWorkflowId = useMemo(() => {
    return workflowIdValue || '';
  }, [workflowIdValue]);

  // Use version value directly (no JSON parsing needed)
  const currentVersion = useMemo(() => {
    return versionValue || 'latest';
  }, [versionValue]);

  const workflowOptions = useMemo(
    () =>
      workflows.map((workflow: WorkflowDto) => ({
        ...workflow,
        label: workflow.name,
        value: workflow.id,
      })),
    [workflows]
  );

  // Check if the current workflow exists
  const workflowExists = useMemo(() => {
    if (!currentWorkflowId) return true;
    return workflows.some((workflow) => workflow.id === currentWorkflowId);
  }, [currentWorkflowId, workflows]);

  // Fetch workflow versions when a workflow is selected
  const { data: versionsResponse, isLoading: isLoadingVersions } =
    useCustomQuery({
      queryKey: queryKeys.workflows.versions(currentWorkflowId ?? ''),
      queryFn: (token: string) =>
        getWorkflowVersions(token, currentWorkflowId!),
      enabled: !!currentWorkflowId,
    });

  const versions = useMemo(() => {
    // Handle wrapped response format
    let versionList: any[] = [];
    if (versionsResponse?.data) {
      versionList = Array.isArray(versionsResponse.data)
        ? versionsResponse.data
        : [];
    } else if (Array.isArray(versionsResponse)) {
      versionList = versionsResponse;
    }

    // Sort versions in reverse chronological order (newest first)
    return versionList.sort((a, b) => {
      const versionA = a.versionNumber || 0;
      const versionB = b.versionNumber || 0;
      return versionB - versionA; // Descending order
    });
  }, [versionsResponse]);

  const handleWorkflowChange = (newWorkflowId: string) => {
    onWorkflowIdChangeRef.current(newWorkflowId);
    // Reset version to latest when workflow changes
    onVersionChangeRef.current('latest');
  };

  const handleVersionChange = (value: string) => {
    // Value can be "latest", "current", or a version number string
    // Store as string to maintain consistency
    onVersionChangeRef.current(value);
  };

  return (
    <div className="space-y-4">
      {/* Workflow Selector */}
      <div>
        <Label className="text-sm font-medium mb-2 block">Child Workflow</Label>
        <div className="flex items-center gap-2 w-full">
          <div className="flex-grow">
            <Select
              onValueChange={handleWorkflowChange}
              value={currentWorkflowId}
              disabled={disabled}
            >
              <SelectTrigger
                className={cn(!workflowExists && 'border-orange-500')}
              >
                <SelectValue placeholder="Select a workflow" />
              </SelectTrigger>
              <SelectContent>
                <SelectGroup>
                  {workflowOptions.map((option: any) => (
                    <SelectItem key={option.value} value={option.value}>
                      {option.label}
                    </SelectItem>
                  ))}
                </SelectGroup>
              </SelectContent>
            </Select>
          </div>
          {!workflowExists && (
            <div className="text-orange-500" title="Workflow not found">
              <Icons.warning className="h-4 w-4" />
            </div>
          )}
        </div>
      </div>

      {/* Version Selector */}
      {currentWorkflowId && (
        <div>
          <Label className="text-sm font-medium mb-2 block">Version</Label>
          {isLoadingVersions ? (
            <div className="text-sm text-muted-foreground">
              Loading versions...
            </div>
          ) : (
            <Select
              value={
                currentVersion === 'latest'
                  ? 'latest'
                  : currentVersion === 'current'
                    ? 'current'
                    : currentVersion?.toString()
              }
              onValueChange={handleVersionChange}
              disabled={disabled}
            >
              <SelectTrigger className="w-full">
                <SelectValue placeholder="Select version" />
              </SelectTrigger>
              <SelectContent>
                <SelectGroup>
                  {/* Latest option always at the top */}
                  <SelectItem value="latest">
                    <span className="font-medium">Latest</span>
                    <span className="text-muted-foreground ml-2">
                      (always use the newest version)
                    </span>
                  </SelectItem>

                  {/* Current option */}
                  <SelectItem value="current">
                    <span className="font-medium">Current</span>
                    <span className="text-muted-foreground ml-2">
                      (locked to version at workflow creation)
                    </span>
                  </SelectItem>

                  {/* Specific versions sorted newest first */}
                  {versions.length > 0 &&
                    versions.map((version: any) => (
                      <SelectItem
                        key={version.versionId || version.versionNumber}
                        value={version.versionNumber?.toString() || '1'}
                      >
                        <span className="font-medium">
                          Version {version.versionNumber || 'Unknown'}
                        </span>
                        {version.createdAt && (
                          <span className="text-muted-foreground ml-2">
                            ({new Date(version.createdAt).toLocaleDateString()})
                          </span>
                        )}
                      </SelectItem>
                    ))}
                </SelectGroup>
              </SelectContent>
            </Select>
          )}
        </div>
      )}
    </div>
  );
}
