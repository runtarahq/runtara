import { useContext, useEffect, useMemo, useState } from 'react';
import {
  Bot,
  ChevronLeft,
  ChevronRight,
  Loader2,
  Search,
  X,
  Zap,
  type LucideIcon,
} from 'lucide-react';
import {
  Dialog,
  DialogContent,
  DialogTitle,
} from '@/shared/components/ui/dialog';
import { VisuallyHidden } from '@radix-ui/react-visually-hidden';
import { Input } from '@/shared/components/ui/input';
import { cn } from '@/lib/utils';
import { ExtendedAgent } from '@/features/workflows/queries';
import { CapabilityInfo, StepTypeInfo } from '@/generated/RuntaraRuntimeApi';
import { NodeFormContext } from './NodeFormContext';
import { useMultipleAgentDetails } from '@/features/workflows/hooks';
import { StepTypeIcon } from '@/features/workflows/components/StepTypeIcon';
import { getAgentIcon } from '@/features/workflows/utils/agent-icons';

interface CapabilitySearchResult {
  agentId: string;
  agentName: string;
  agentIcon: LucideIcon;
  capability: CapabilityInfo;
  isSupported: boolean;
}

type ViewMode = 'browse' | 'search' | 'capabilities';

export interface StepPickerResult {
  stepType: string;
  agentId?: string;
  capabilityId?: string;
  name: string;
}

interface StepPickerModalProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSelect: (result: StepPickerResult) => void;
  allowFinish?: boolean;
  mode?: 'all' | 'tool' | 'memory';
  closeOnSelect?: boolean;
}

interface StepPickerPanelProps {
  active?: boolean;
  onSelect: (result: StepPickerResult) => void;
  onCancel?: () => void;
  allowFinish?: boolean;
  mode?: 'all' | 'tool' | 'memory';
  contentScrollable?: boolean;
  autoFocus?: boolean;
}

const TOOL_STEP_TYPES = new Set([
  'waitforsignal',
  'wait for signal',
  'startworkflow',
  'start workflow',
]);

function toTestIdPart(value: string | undefined): string {
  return (value || 'unknown')
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-|-$/g, '');
}

function normalizeStepType(stepType: StepTypeInfo): string {
  if (stepType.id) return stepType.id;

  const name = stepType.name || '';
  const knownMappings: Record<string, string> = {
    'AI Agent': 'AiAgent',
    'Wait for Signal': 'WaitForSignal',
  };
  return knownMappings[name] ?? name.replace(/\s+/g, '');
}

export function StepPickerPanel({
  active = true,
  onSelect,
  onCancel,
  allowFinish = true,
  mode = 'all',
  contentScrollable = true,
  autoFocus = true,
}: StepPickerPanelProps) {
  const { agents, stepTypes } = useContext(NodeFormContext);
  const [viewMode, setViewMode] = useState<ViewMode>('browse');
  const [selectedAgent, setSelectedAgent] = useState<{
    id: string;
    name: string;
  } | null>(null);
  const [searchQuery, setSearchQuery] = useState('');

  const filteredStepTypes = useMemo(() => {
    return (stepTypes || []).filter((stepType) => {
      if (stepType.name === 'Start' || stepType.name === 'Agent') return false;
      if (stepType.name === 'Finish' && !allowFinish) return false;
      if (
        mode === 'tool' &&
        !TOOL_STEP_TYPES.has((stepType.name || '').toLowerCase())
      ) {
        return false;
      }
      if (mode === 'memory') return false;
      return true;
    });
  }, [stepTypes, allowFinish, mode]);

  const filteredAgents = useMemo(() => {
    const allAgents = (agents || []) as ExtendedAgent[];
    if (mode !== 'memory') return allAgents;

    return allAgents.filter((agent) => {
      let hasRead = false;
      let hasWrite = false;

      for (const capability of Object.values(agent.supportedCapabilities)) {
        if (capability.tags?.includes('memory:read')) hasRead = true;
        if (capability.tags?.includes('memory:write')) hasWrite = true;
      }

      return hasRead && hasWrite;
    });
  }, [agents, mode]);

  const agentIds = useMemo(
    () => ((agents || []) as ExtendedAgent[]).map((agent) => agent.id || ''),
    [agents]
  );

  const {
    agentDetailsMap,
    allLoaded: allAgentsLoaded,
    isLoading: someLoading,
  } = useMultipleAgentDetails(agentIds, { enabled: active });

  const agentCapabilitiesMap = useMemo(() => {
    const map = new Map<
      string,
      {
        details: ReturnType<typeof agentDetailsMap.get>;
        capabilities: CapabilityInfo[];
      }
    >();

    for (const [agentId, details] of agentDetailsMap) {
      if (details) {
        map.set(agentId, {
          details,
          capabilities: details.capabilities || [],
        });
      }
    }

    return map;
  }, [agentDetailsMap]);

  const selectedAgentData = useMemo(() => {
    if (!selectedAgent) return null;
    return agentCapabilitiesMap.get(selectedAgent.id);
  }, [selectedAgent, agentCapabilitiesMap]);

  useEffect(() => {
    if (!active) {
      setViewMode('browse');
      setSelectedAgent(null);
      setSearchQuery('');
    }
  }, [active]);

  useEffect(() => {
    if (searchQuery.trim()) {
      setViewMode('search');
    } else if (viewMode === 'search') {
      setViewMode('browse');
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- viewMode is read but should not trigger this effect
  }, [searchQuery]);

  const searchResults = useMemo(() => {
    if (!searchQuery.trim()) {
      return { stepTypes: [], agents: [], capabilities: [] };
    }

    const query = searchQuery.toLowerCase();
    const searchableAgents =
      mode === 'memory' ? filteredAgents : ((agents || []) as ExtendedAgent[]);

    const matchingStepTypes = filteredStepTypes.filter(
      (stepType) =>
        stepType.name?.toLowerCase().includes(query) ||
        stepType.description?.toLowerCase().includes(query)
    );
    const matchingAgents = searchableAgents.filter(
      (agent) =>
        agent.name?.toLowerCase().includes(query) ||
        agent.description?.toLowerCase().includes(query)
    );
    const matchingCapabilities: CapabilitySearchResult[] = [];

    if (allAgentsLoaded) {
      for (const agent of searchableAgents) {
        const agentData = agentCapabilitiesMap.get(agent.id || '');
        if (!agentData) continue;

        for (const capability of agentData.capabilities) {
          const matchesSearch =
            capability.name?.toLowerCase().includes(query) ||
            capability.displayName?.toLowerCase().includes(query) ||
            capability.description?.toLowerCase().includes(query);

          if (matchesSearch) {
            matchingCapabilities.push({
              agentId: agent.id || '',
              agentName: agent.name || '',
              agentIcon: getAgentIcon(agent.id),
              capability,
              isSupported: true,
            });
          }
        }
      }
    }

    return {
      stepTypes: matchingStepTypes,
      agents: matchingAgents,
      capabilities: mode === 'memory' ? [] : matchingCapabilities,
    };
  }, [
    searchQuery,
    filteredStepTypes,
    agents,
    filteredAgents,
    agentCapabilitiesMap,
    allAgentsLoaded,
    mode,
  ]);

  const handleAgentSelect = (agentId: string, agentName: string) => {
    if (mode === 'memory') {
      let capabilityId: string | undefined;
      const agentData = agentCapabilitiesMap.get(agentId);

      if (agentData?.capabilities.length) {
        const bothTags = agentData.capabilities.find(
          (capability) =>
            capability.tags?.includes('memory:read') &&
            capability.tags?.includes('memory:write')
        );
        const anyMemoryTag = agentData.capabilities.find((capability) =>
          capability.tags?.some((tag) => tag.startsWith('memory:'))
        );
        capabilityId =
          bothTags?.id ?? anyMemoryTag?.id ?? agentData.capabilities[0]?.id;
      }

      if (!capabilityId) {
        const agent = filteredAgents.find(
          (candidate) => candidate.id === agentId
        );
        if (agent) {
          const capabilities = Object.values(agent.supportedCapabilities);
          const bothTags = capabilities.find(
            (capability) =>
              capability.tags?.includes('memory:read') &&
              capability.tags?.includes('memory:write')
          );
          const anyMemoryTag = capabilities.find((capability) =>
            capability.tags?.some((tag) => tag.startsWith('memory:'))
          );
          capabilityId =
            bothTags?.id ?? anyMemoryTag?.id ?? capabilities[0]?.id;
        }
      }

      onSelect({
        stepType: 'Agent',
        agentId,
        capabilityId: capabilityId || '',
        name: agentName,
      });
      return;
    }

    setSelectedAgent({ id: agentId, name: agentName });
    setViewMode('capabilities');
    setSearchQuery('');
  };

  const handleStepTypeSelect = (stepType: StepTypeInfo) => {
    onSelect({
      stepType: normalizeStepType(stepType),
      name: stepType.name || '',
    });
  };

  const handleCapabilitySelect = (agentId: string, capabilityId: string) => {
    const agentData = agentCapabilitiesMap.get(agentId);
    const capability = agentData?.capabilities.find(
      (candidate) => candidate.id === capabilityId
    );

    onSelect({
      stepType: 'Agent',
      agentId,
      capabilityId,
      name: capability?.displayName || capability?.name || 'New capability',
    });
  };

  const handleBack = () => {
    setViewMode('browse');
    setSelectedAgent(null);
    setSearchQuery('');
  };

  const title =
    viewMode === 'search'
      ? 'Search Results'
      : viewMode === 'capabilities'
        ? `${selectedAgent?.name} Capabilities`
        : mode === 'tool'
          ? 'Add Tool'
          : mode === 'memory'
            ? 'Add Memory Provider'
            : 'Add Step';
  const subtitle =
    viewMode === 'search'
      ? 'Matching steps and capabilities'
      : viewMode === 'capabilities'
        ? 'Choose a capability to perform'
        : mode === 'tool'
          ? 'Choose agent capability, WaitForSignal, or EmbedWorkflow'
          : mode === 'memory'
            ? 'Choose an agent to provide memory'
            : 'Search or browse step types and agents';

  return (
    <>
      <div className="flex items-center gap-2 border-b p-4">
        {viewMode === 'capabilities' && (
          <button
            type="button"
            onClick={handleBack}
            className="rounded p-1 hover:bg-muted"
            aria-label="Back to step list"
          >
            <ChevronLeft className="h-4 w-4" />
          </button>
        )}
        <div className="flex-1">
          <h2 className="text-lg font-semibold">{title}</h2>
          <p className="text-sm text-muted-foreground">{subtitle}</p>
        </div>
        {onCancel && (
          <button
            type="button"
            onClick={onCancel}
            className="rounded p-1 hover:bg-muted"
            aria-label="Cancel adding step"
          >
            <X className="h-4 w-4" />
          </button>
        )}
      </div>

      <div className="border-b p-4">
        <div className="relative">
          <Search className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
          <Input
            placeholder="Search steps or operations..."
            value={searchQuery}
            onChange={(event) => setSearchQuery(event.target.value)}
            className="pl-9"
            autoFocus={autoFocus}
          />
        </div>
      </div>

      <div
        className={cn(
          'p-4',
          contentScrollable && 'max-h-[400px] overflow-y-auto'
        )}
      >
        {viewMode === 'search' ? (
          <SearchResults
            loading={someLoading}
            searchQuery={searchQuery}
            results={searchResults}
            onStepTypeSelect={handleStepTypeSelect}
            onAgentSelect={handleAgentSelect}
            onCapabilitySelect={handleCapabilitySelect}
          />
        ) : viewMode === 'capabilities' ? (
          <CapabilityList
            selectedAgentId={selectedAgent?.id}
            selectedAgentData={selectedAgentData}
            onCapabilitySelect={handleCapabilitySelect}
          />
        ) : (
          <BrowseList
            mode={mode}
            stepTypes={filteredStepTypes}
            agents={filteredAgents}
            onStepTypeSelect={handleStepTypeSelect}
            onAgentSelect={handleAgentSelect}
          />
        )}
      </div>
    </>
  );
}

function SearchResults({
  loading,
  searchQuery,
  results,
  onStepTypeSelect,
  onAgentSelect,
  onCapabilitySelect,
}: {
  loading: boolean;
  searchQuery: string;
  results: {
    stepTypes: StepTypeInfo[];
    agents: ExtendedAgent[];
    capabilities: CapabilitySearchResult[];
  };
  onStepTypeSelect: (stepType: StepTypeInfo) => void;
  onAgentSelect: (agentId: string, agentName: string) => void;
  onCapabilitySelect: (agentId: string, capabilityId: string) => void;
}) {
  const hasResults =
    results.stepTypes.length > 0 ||
    results.agents.length > 0 ||
    results.capabilities.length > 0;

  if (!hasResults) {
    return (
      <div className="space-y-4">
        {loading && <LoadingMore />}
        <div className="py-8 text-center text-muted-foreground">
          No results found for "{searchQuery}"
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-4">
      {loading && <LoadingMore />}
      {results.stepTypes.length > 0 && (
        <StepTypeSection
          stepTypes={results.stepTypes}
          onStepTypeSelect={onStepTypeSelect}
        />
      )}
      {results.agents.length > 0 && (
        <AgentSection agents={results.agents} onAgentSelect={onAgentSelect} />
      )}
      {results.capabilities.length > 0 && (
        <CapabilitySearchSection
          results={results.capabilities}
          onCapabilitySelect={onCapabilitySelect}
        />
      )}
    </div>
  );
}

function BrowseList({
  mode,
  stepTypes,
  agents,
  onStepTypeSelect,
  onAgentSelect,
}: {
  mode: 'all' | 'tool' | 'memory';
  stepTypes: StepTypeInfo[];
  agents: ExtendedAgent[];
  onStepTypeSelect: (stepType: StepTypeInfo) => void;
  onAgentSelect: (agentId: string, agentName: string) => void;
}) {
  return (
    <div className="space-y-6">
      {stepTypes.length > 0 && (
        <StepTypeSection
          stepTypes={stepTypes}
          onStepTypeSelect={onStepTypeSelect}
        />
      )}
      {agents.length > 0 && (
        <AgentSection
          title={mode === 'memory' ? 'Memory Providers' : 'Agents'}
          agents={agents}
          onAgentSelect={onAgentSelect}
        />
      )}
    </div>
  );
}

function StepTypeSection({
  stepTypes,
  onStepTypeSelect,
}: {
  stepTypes: StepTypeInfo[];
  onStepTypeSelect: (stepType: StepTypeInfo) => void;
}) {
  return (
    <div>
      <div className="mb-2 flex items-center gap-2">
        <Zap className="h-5 w-5 text-muted-foreground" />
        <span className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          Step Types
        </span>
      </div>
      <div className="space-y-1">
        {stepTypes.map((stepType) => (
          <button
            key={stepType.name}
            type="button"
            onClick={() => onStepTypeSelect(stepType)}
            data-testid={`step-picker-step-type-${toTestIdPart(normalizeStepType(stepType))}`}
            className="flex w-full items-center gap-3 rounded-lg px-3 py-2 text-left transition-colors hover:bg-muted"
          >
            <StepTypeIcon
              className="h-5 w-5 text-muted-foreground"
              type={stepType.name || ''}
            />
            <div>
              <div className="font-medium">{stepType.name}</div>
              {stepType.description && (
                <div
                  className="line-clamp-2 text-xs text-muted-foreground"
                  title={stepType.description}
                >
                  {stepType.description}
                </div>
              )}
            </div>
          </button>
        ))}
      </div>
    </div>
  );
}

function AgentSection({
  title = 'Agents',
  agents,
  onAgentSelect,
}: {
  title?: string;
  agents: ExtendedAgent[];
  onAgentSelect: (agentId: string, agentName: string) => void;
}) {
  return (
    <div>
      <div className="mb-2 flex items-center gap-2">
        <Bot className="h-5 w-5 text-muted-foreground" />
        <span className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          {title}
        </span>
      </div>
      <div className="space-y-1">
        {agents.map((agent) => {
          const AgentIcon = getAgentIcon(agent.id);

          return (
            <button
              key={agent.id}
              type="button"
              onClick={() => onAgentSelect(agent.id || '', agent.name || '')}
              data-testid={`step-picker-agent-${toTestIdPart(agent.id)}`}
              className="flex w-full items-center justify-between gap-2 rounded-lg px-3 py-2 text-left transition-colors hover:bg-muted"
            >
              <div className="flex items-center gap-3">
                <div className="flex h-5 w-5 shrink-0 items-center justify-center">
                  <AgentIcon className="h-5 w-5 text-muted-foreground" />
                </div>
                <div>
                  <div className="font-medium">{agent.name}</div>
                  {agent.description && (
                    <div
                      className="line-clamp-2 text-xs text-muted-foreground"
                      title={agent.description}
                    >
                      {agent.description}
                    </div>
                  )}
                </div>
              </div>
              <ChevronRight className="h-4 w-4 shrink-0 text-muted-foreground" />
            </button>
          );
        })}
      </div>
    </div>
  );
}

function CapabilitySearchSection({
  results,
  onCapabilitySelect,
}: {
  results: CapabilitySearchResult[];
  onCapabilitySelect: (agentId: string, capabilityId: string) => void;
}) {
  return (
    <div>
      <div className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
        Capabilities
      </div>
      <div className="space-y-1">
        {results.map((result) => {
          const AgentIcon = result.agentIcon;

          return (
            <button
              key={`${result.agentId}-${result.capability.id}`}
              type="button"
              onClick={() =>
                result.isSupported &&
                onCapabilitySelect(result.agentId, result.capability.id)
              }
              disabled={!result.isSupported}
              data-testid={`step-picker-capability-${toTestIdPart(result.agentId)}-${toTestIdPart(result.capability.id)}`}
              className={cn(
                'flex w-full flex-col gap-1 rounded-lg px-3 py-3 text-left transition-colors',
                result.isSupported
                  ? 'hover:bg-muted'
                  : 'cursor-not-allowed opacity-50'
              )}
            >
              <div className="flex items-center gap-2">
                <div className="flex h-4 w-4 shrink-0 items-center justify-center">
                  <AgentIcon className="h-3.5 w-3.5 text-muted-foreground" />
                </div>
                <span className="text-xs text-muted-foreground">
                  {result.agentName}
                </span>
                <span className="text-muted-foreground">→</span>
                <span className="font-medium">
                  {result.capability.displayName || result.capability.name}
                </span>
                {!result.isSupported && (
                  <span className="rounded bg-yellow-100 px-1.5 py-0.5 text-[10px] font-medium text-yellow-700">
                    Coming Soon
                  </span>
                )}
              </div>
              {result.capability.description && (
                <p
                  className="ml-6 line-clamp-2 text-xs text-muted-foreground"
                  title={result.capability.description}
                >
                  {result.capability.description}
                </p>
              )}
            </button>
          );
        })}
      </div>
    </div>
  );
}

function CapabilityList({
  selectedAgentId,
  selectedAgentData,
  onCapabilitySelect,
}: {
  selectedAgentId?: string;
  selectedAgentData?: { capabilities: CapabilityInfo[] } | null;
  onCapabilitySelect: (agentId: string, capabilityId: string) => void;
}) {
  if (!selectedAgentData || !selectedAgentId) {
    return (
      <div className="flex items-center justify-center py-8">
        <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
        <span className="ml-2 text-muted-foreground">
          Loading capabilities...
        </span>
      </div>
    );
  }

  if ((selectedAgentData.capabilities || []).length === 0) {
    return (
      <div className="py-8 text-center text-muted-foreground">
        No capabilities found
      </div>
    );
  }

  return (
    <div className="space-y-1">
      {(selectedAgentData.capabilities || []).map((capability) => (
        <button
          key={capability.id}
          type="button"
          onClick={() => onCapabilitySelect(selectedAgentId, capability.id)}
          data-testid={`step-picker-capability-${toTestIdPart(selectedAgentId)}-${toTestIdPart(capability.id)}`}
          className="flex w-full flex-col gap-1 rounded-lg px-3 py-3 text-left transition-colors hover:bg-muted"
        >
          <div className="flex items-center gap-2">
            <span className="font-medium">
              {capability.displayName || capability.name}
            </span>
          </div>
          {capability.description && (
            <p
              className="line-clamp-2 text-xs text-muted-foreground"
              title={capability.description}
            >
              {capability.description}
            </p>
          )}
        </button>
      ))}
    </div>
  );
}

function LoadingMore() {
  return (
    <div className="flex items-center gap-2 text-sm text-muted-foreground">
      <Loader2 className="h-4 w-4 animate-spin" />
      Loading more results...
    </div>
  );
}

export function StepPickerModal({
  open,
  onOpenChange,
  onSelect,
  allowFinish = true,
  mode = 'all',
  closeOnSelect = true,
}: StepPickerModalProps) {
  const handleSelect = (result: StepPickerResult) => {
    onSelect(result);
    if (closeOnSelect) {
      onOpenChange(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className="sm:max-w-[500px] p-0 gap-0"
        hideCloseButton
        aria-describedby={undefined}
      >
        <VisuallyHidden>
          <DialogTitle>Add Step</DialogTitle>
        </VisuallyHidden>
        <StepPickerPanel
          active={open}
          onSelect={handleSelect}
          allowFinish={allowFinish}
          mode={mode}
        />
      </DialogContent>
    </Dialog>
  );
}
