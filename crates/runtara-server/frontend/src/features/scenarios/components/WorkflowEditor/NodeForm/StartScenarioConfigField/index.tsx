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
import { ScenarioDto } from '@/generated/RuntaraRuntimeApi';
import { NodeFormContext } from '../NodeFormContext.tsx';
import { Icons } from '@/shared/components/icons.tsx';
import { cn } from '@/lib/utils.ts';
import { useCustomQuery } from '@/shared/hooks/api';
import { getScenarioVersions } from '@/features/scenarios/queries';
import { queryKeys } from '@/shared/queries/query-keys';

interface StartScenarioConfigFieldProps {
  scenarioIdValue: string;
  versionValue: string;
  onScenarioIdChange: (value: string) => void;
  onVersionChange: (value: string) => void;
  disabled?: boolean;
}

export function StartScenarioConfigField({
  scenarioIdValue,
  versionValue,
  onScenarioIdChange,
  onVersionChange,
  disabled = false,
}: StartScenarioConfigFieldProps) {
  const { scenarios } = useContext(NodeFormContext);

  // Use refs to store callbacks to avoid dependency issues
  const onScenarioIdChangeRef = useRef(onScenarioIdChange);
  const onVersionChangeRef = useRef(onVersionChange);

  useEffect(() => {
    onScenarioIdChangeRef.current = onScenarioIdChange;
    onVersionChangeRef.current = onVersionChange;
  }, [onScenarioIdChange, onVersionChange]);

  // Use scenario ID directly (no JSON parsing needed)
  const currentScenarioId = useMemo(() => {
    return scenarioIdValue || '';
  }, [scenarioIdValue]);

  // Use version value directly (no JSON parsing needed)
  const currentVersion = useMemo(() => {
    return versionValue || 'latest';
  }, [versionValue]);

  const scenarioOptions = useMemo(
    () =>
      scenarios.map((scenario: ScenarioDto) => ({
        ...scenario,
        label: scenario.name,
        value: scenario.id,
      })),
    [scenarios]
  );

  // Check if the current scenario exists
  const scenarioExists = useMemo(() => {
    if (!currentScenarioId) return true;
    return scenarios.some((scenario) => scenario.id === currentScenarioId);
  }, [currentScenarioId, scenarios]);

  // Fetch scenario versions when a scenario is selected
  const { data: versionsResponse, isLoading: isLoadingVersions } =
    useCustomQuery({
      queryKey: queryKeys.scenarios.versions(currentScenarioId ?? ''),
      queryFn: (token: string) =>
        getScenarioVersions(token, currentScenarioId!),
      enabled: !!currentScenarioId,
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

  const handleScenarioChange = (newScenarioId: string) => {
    onScenarioIdChangeRef.current(newScenarioId);
    // Reset version to latest when scenario changes
    onVersionChangeRef.current('latest');
  };

  const handleVersionChange = (value: string) => {
    // Value can be "latest", "current", or a version number string
    // Store as string to maintain consistency
    onVersionChangeRef.current(value);
  };

  return (
    <div className="space-y-4">
      {/* Scenario Selector */}
      <div>
        <Label className="text-sm font-medium mb-2 block">Child Scenario</Label>
        <div className="flex items-center gap-2 w-full">
          <div className="flex-grow">
            <Select
              onValueChange={handleScenarioChange}
              value={currentScenarioId}
              disabled={disabled}
            >
              <SelectTrigger
                className={cn(!scenarioExists && 'border-orange-500')}
              >
                <SelectValue placeholder="Select a scenario" />
              </SelectTrigger>
              <SelectContent>
                <SelectGroup>
                  {scenarioOptions.map((option: any) => (
                    <SelectItem key={option.value} value={option.value}>
                      {option.label}
                    </SelectItem>
                  ))}
                </SelectGroup>
              </SelectContent>
            </Select>
          </div>
          {!scenarioExists && (
            <div className="text-orange-500" title="Scenario not found">
              <Icons.warning className="h-4 w-4" />
            </div>
          )}
        </div>
      </div>

      {/* Version Selector */}
      {currentScenarioId && (
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
                      (locked to version at scenario creation)
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
