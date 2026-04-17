import { useState, useMemo, useContext, useEffect } from 'react';
import {
  Bot,
  ChevronLeft,
  ChevronRight,
  Search,
  Loader2,
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
import { ExtendedAgent } from '@/features/scenarios/queries';
import {
  CapabilityInfo,
  StepTypeInfo,
} from '@/generated/RuntaraRuntimeApi';
import { NodeFormContext } from './NodeFormContext';
import { useMultipleAgentDetails } from '@/features/scenarios/hooks';
import { StepTypeIcon } from '@/features/scenarios/components/StepTypeIcon';
import { getAgentIcon } from '@/features/scenarios/utils/agent-icons';

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
  /** Allow selecting Finish step (for outgoing connections). Defaults to true. */
  allowFinish?: boolean;
  /**
   * Filter mode for the picker:
   * - 'all' (default): show everything
   * - 'tool': only Agent capabilities + WaitForSignal, StartScenario step types
   * - 'memory': only Agent capabilities (memory providers)
   */
  mode?: 'all' | 'tool' | 'memory';
}

// Normalize step type names from backend format (with spaces) to internal camelCase
function normalizeStepType(stepType: string): string {
  // Map known backend formats that don't normalize correctly with simple space removal
  const knownMappings: Record<string, string> = {
    'AI Agent': 'AiAgent',
    'Wait for Signal': 'WaitForSignal',
  };
  return knownMappings[stepType] ?? stepType.replace(/\s+/g, '');
}

/**
 * Unified modal for selecting step type and operation with searchable interface
 * Combines step type selection and operator/operation selection into one flow
 */
// Step types allowed when picking a tool for an AI Agent (lowercase for case-insensitive matching)
const TOOL_STEP_TYPES = new Set([
  'waitforsignal',
  'wait for signal',
  'startscenario',
  'start scenario',
]);

export function StepPickerModal({
  open,
  onOpenChange,
  onSelect,
  allowFinish = true,
  mode = 'all',
}: StepPickerModalProps) {
  const { agents, stepTypes } = useContext(NodeFormContext);
  const [viewMode, setViewMode] = useState<ViewMode>('browse');
  const [selectedAgent, setSelectedAgent] = useState<{
    id: string;
    name: string;
  } | null>(null);
  const [searchQuery, setSearchQuery] = useState('');

  // Filter out deprecated and invalid step types
  // - Start: deprecated (cannot be added manually)
  // - Agent: deprecated (use operators instead)
  // - Finish: optionally allowed for outgoing connections
  const filteredStepTypes = useMemo(() => {
    return (stepTypes || []).filter((st) => {
      if (st.name === 'Start' || st.name === 'Agent') return false;
      if (st.name === 'Finish' && !allowFinish) return false;
      // In tool mode, only allow WaitForSignal and StartScenario
      if (
        mode === 'tool' &&
        !TOOL_STEP_TYPES.has((st.name || '').toLowerCase())
      )
        return false;
      // In memory mode, no step types — only agents
      if (mode === 'memory') return false;
      return true;
    });
  }, [stepTypes, allowFinish, mode]);

  // In memory mode, filter to only agents that have both memory:read and memory:write tags
  const filteredAgents = useMemo(() => {
    const all = (agents || []) as ExtendedAgent[];
    if (mode !== 'memory') return all;
    return all.filter((agent) => {
      let hasRead = false;
      let hasWrite = false;
      for (const cap of Object.values(agent.supportedCapabilities)) {
        if (cap.tags?.includes('memory:read')) hasRead = true;
        if (cap.tags?.includes('memory:write')) hasWrite = true;
      }
      return hasRead && hasWrite;
    });
  }, [agents, mode]);

  // Get agent IDs for fetching details
  const agentIds = useMemo(
    () => ((agents || []) as ExtendedAgent[]).map((a) => a.id || ''),
    [agents]
  );

  // Fetch details for ALL agents to enable global search
  const {
    agentDetailsMap,
    allLoaded: allAgentsLoaded,
    isLoading: someLoading,
  } = useMultipleAgentDetails(agentIds, { enabled: open });

  // Build a map of agent id to their capabilities
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

  // Fetch agent details when agent is selected
  const selectedAgentData = useMemo(() => {
    if (!selectedAgent) return null;
    return agentCapabilitiesMap.get(selectedAgent.id);
  }, [selectedAgent, agentCapabilitiesMap]);

  // Reset state when modal closes
  const handleOpenChange = (newOpen: boolean) => {
    onOpenChange(newOpen);
    if (!newOpen) {
      setViewMode('browse');
      setSelectedAgent(null);
      setSearchQuery('');
    }
  };

  // Update view mode based on search query
  useEffect(() => {
    if (searchQuery.trim()) {
      setViewMode('search');
    } else if (viewMode === 'search') {
      setViewMode('browse');
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- viewMode is read but shouldn't trigger re-run
  }, [searchQuery]);

  // Global search across step types, agents and capabilities
  const searchResults = useMemo(() => {
    if (!searchQuery.trim())
      return { stepTypes: [], agents: [], capabilities: [] };

    const query = searchQuery.toLowerCase();

    // Search step types (excluding Agent and Start)
    const matchingStepTypes = filteredStepTypes.filter(
      (st) =>
        st.name?.toLowerCase().includes(query) ||
        st.description?.toLowerCase().includes(query)
    );

    // Search agents (in memory mode, only search memory-capable agents)
    const searchableAgents =
      mode === 'memory' ? filteredAgents : ((agents || []) as ExtendedAgent[]);
    const matchingAgents = searchableAgents.filter(
      (ag) =>
        ag.name?.toLowerCase().includes(query) ||
        ag.description?.toLowerCase().includes(query)
    );

    // Search capabilities across all agents (in memory mode, only memory-capable agents)
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
    // In memory mode, auto-select the best memory capability — never drill into capabilities
    if (mode === 'memory') {
      let capId: string | undefined;

      // 1. Try detailed capabilities: prefer cap with both tags, fall back to any memory tag
      const agentData = agentCapabilitiesMap.get(agentId);
      if (agentData?.capabilities.length) {
        const bothTags = agentData.capabilities.find((cap) => {
          const tags = (cap as any).tags as string[] | undefined;
          return (
            tags?.includes('memory:read') && tags?.includes('memory:write')
          );
        });
        if (bothTags) {
          capId = bothTags.id;
        } else {
          const anyMemoryTag = agentData.capabilities.find((cap) => {
            const tags = (cap as any).tags as string[] | undefined;
            return tags?.some((t: string) => t.startsWith('memory:'));
          });
          capId = anyMemoryTag?.id ?? agentData.capabilities[0]?.id;
        }
      }

      // 2. Fallback to supportedCapabilities (basic agent info)
      if (!capId) {
        const agent = filteredAgents.find((a) => a.id === agentId);
        if (agent) {
          const caps = Object.values(agent.supportedCapabilities);
          const bothTags = caps.find((cap) => {
            const tags = (cap as any).tags as string[] | undefined;
            return (
              tags?.includes('memory:read') && tags?.includes('memory:write')
            );
          });
          if (bothTags) {
            capId = bothTags.id;
          } else {
            const anyMemoryTag = caps.find((cap) => {
              const tags = (cap as any).tags as string[] | undefined;
              return tags?.some((t: string) => t.startsWith('memory:'));
            });
            capId = anyMemoryTag?.id ?? caps[0]?.id;
          }
        }
      }

      onSelect({
        stepType: 'Agent',
        agentId,
        capabilityId: capId || '',
        name: agentName,
      });
      handleOpenChange(false);
      return;
    }

    setSelectedAgent({ id: agentId, name: agentName });
    setViewMode('capabilities');
    setSearchQuery('');
  };

  const handleStepTypeSelect = (stepType: StepTypeInfo) => {
    onSelect({
      stepType: normalizeStepType(stepType.name || ''),
      name: stepType.name || '',
    });
    handleOpenChange(false);
  };

  const handleCapabilitySelect = (agentId: string, capabilityId: string) => {
    const agentData = agentCapabilitiesMap.get(agentId);
    const capability = agentData?.capabilities.find(
      (cap) => cap.id === capabilityId
    );

    onSelect({
      stepType: 'Agent',
      agentId: agentId,
      capabilityId: capabilityId,
      name: capability?.displayName || capability?.name || 'New capability',
    });
    handleOpenChange(false);
  };

  const handleBack = () => {
    setViewMode('browse');
    setSelectedAgent(null);
    setSearchQuery('');
  };

  const getTitle = () => {
    if (viewMode === 'search') return 'Search Results';
    if (viewMode === 'capabilities')
      return `${selectedAgent?.name} Capabilities`;
    if (mode === 'tool') return 'Add Tool';
    if (mode === 'memory') return 'Add Memory Provider';
    return 'Add Step';
  };

  const getSubtitle = () => {
    if (viewMode === 'search') return 'Matching steps and capabilities';
    if (viewMode === 'capabilities') return 'Choose a capability to perform';
    if (mode === 'tool')
      return 'Choose agent capability, WaitForSignal, or StartScenario';
    if (mode === 'memory') return 'Choose an agent to provide memory';
    return 'Search or browse step types and agents';
  };

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent
        className="sm:max-w-[500px] p-0 gap-0"
        hideCloseButton
        aria-describedby={undefined}
      >
        {/* Visually hidden title for screen readers */}
        <VisuallyHidden>
          <DialogTitle>{getTitle()}</DialogTitle>
        </VisuallyHidden>

        {/* Header */}
        <div className="flex items-center gap-2 p-4 border-b">
          {viewMode === 'capabilities' && (
            <button
              type="button"
              onClick={handleBack}
              className="p-1 rounded hover:bg-muted"
            >
              <ChevronLeft className="h-4 w-4" />
            </button>
          )}
          <div className="flex-1">
            <h2 className="text-lg font-semibold">{getTitle()}</h2>
            <p className="text-sm text-muted-foreground">{getSubtitle()}</p>
          </div>
        </div>

        {/* Search */}
        <div className="p-4 border-b">
          <div className="relative">
            <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
            <Input
              placeholder="Search steps or operations..."
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              className="pl-9"
              autoFocus
            />
          </div>
        </div>

        {/* Content */}
        <div className="max-h-[400px] overflow-y-auto p-4">
          {viewMode === 'search' ? (
            // Search Results View
            <div className="space-y-4">
              {someLoading && (
                <div className="flex items-center gap-2 text-sm text-muted-foreground">
                  <Loader2 className="h-4 w-4 animate-spin" />
                  Loading more results...
                </div>
              )}

              {searchResults.stepTypes.length === 0 &&
              searchResults.agents.length === 0 &&
              searchResults.capabilities.length === 0 ? (
                <div className="text-center py-8 text-muted-foreground">
                  No results found for "{searchQuery}"
                </div>
              ) : (
                <>
                  {/* Matching Step Types */}
                  {searchResults.stepTypes.length > 0 && (
                    <div>
                      <div className="text-xs font-semibold text-muted-foreground uppercase tracking-wide mb-2">
                        Step Types
                      </div>
                      <div className="space-y-1">
                        {searchResults.stepTypes.map((stepType) => (
                          <button
                            key={stepType.name}
                            type="button"
                            onClick={() => handleStepTypeSelect(stepType)}
                            className="w-full flex items-center gap-3 px-3 py-2 rounded-lg text-left transition-colors hover:bg-muted"
                          >
                            <StepTypeIcon
                              className="w-5 h-5 text-muted-foreground"
                              type={stepType.name || ''}
                            />
                            <div>
                              <div className="font-medium">{stepType.name}</div>
                              {stepType.description && (
                                <div
                                  className="text-xs text-muted-foreground line-clamp-2"
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
                  )}

                  {/* Matching Agents */}
                  {searchResults.agents.length > 0 && (
                    <div>
                      <div className="text-xs font-semibold text-muted-foreground uppercase tracking-wide mb-2">
                        Agents
                      </div>
                      <div className="space-y-1">
                        {searchResults.agents.map((agent) => {
                          const AgentIcon = getAgentIcon(agent.id);
                          return (
                            <button
                              key={agent.id}
                              type="button"
                              onClick={() =>
                                handleAgentSelect(
                                  agent.id || '',
                                  agent.name || ''
                                )
                              }
                              className="w-full flex items-center justify-between gap-2 px-3 py-2 rounded-lg text-left transition-colors hover:bg-muted"
                            >
                              <div className="flex items-center gap-3">
                                <div className="w-5 h-5 flex items-center justify-center shrink-0">
                                  <AgentIcon className="w-5 h-5 text-muted-foreground" />
                                </div>
                                <div>
                                  <div className="font-medium">
                                    {agent.name}
                                  </div>
                                  {agent.description && (
                                    <div
                                      className="text-xs text-muted-foreground line-clamp-2"
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
                  )}

                  {/* Matching Capabilities */}
                  {searchResults.capabilities.length > 0 && (
                    <div>
                      <div className="text-xs font-semibold text-muted-foreground uppercase tracking-wide mb-2">
                        Capabilities
                      </div>
                      <div className="space-y-1">
                        {searchResults.capabilities.map((result) => (
                          <button
                            key={`${result.agentId}-${result.capability.id}`}
                            type="button"
                            onClick={() =>
                              result.isSupported &&
                              handleCapabilitySelect(
                                result.agentId,
                                result.capability.id
                              )
                            }
                            disabled={!result.isSupported}
                            className={cn(
                              'w-full flex flex-col gap-1 px-3 py-3 rounded-lg text-left transition-colors',
                              result.isSupported
                                ? 'hover:bg-muted'
                                : 'opacity-50 cursor-not-allowed'
                            )}
                          >
                            <div className="flex items-center gap-2">
                              <div className="w-4 h-4 flex items-center justify-center shrink-0">
                                <result.agentIcon className="w-3.5 h-3.5 text-muted-foreground" />
                              </div>
                              <span className="text-xs text-muted-foreground">
                                {result.agentName}
                              </span>
                              <span className="text-muted-foreground">→</span>
                              <span className="font-medium">
                                {result.capability.displayName ||
                                  result.capability.name}
                              </span>
                              {!result.isSupported && (
                                <span className="px-1.5 py-0.5 text-[10px] font-medium rounded bg-yellow-100 text-yellow-700">
                                  Coming Soon
                                </span>
                              )}
                            </div>
                            {result.capability.description && (
                              <p
                                className="text-xs text-muted-foreground line-clamp-2 ml-6"
                                title={result.capability.description}
                              >
                                {result.capability.description}
                              </p>
                            )}
                          </button>
                        ))}
                      </div>
                    </div>
                  )}
                </>
              )}
            </div>
          ) : viewMode === 'capabilities' ? (
            // Capabilities List View
            !selectedAgentData ? (
              <div className="flex items-center justify-center py-8">
                <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
                <span className="ml-2 text-muted-foreground">
                  Loading capabilities...
                </span>
              </div>
            ) : (
              <div className="space-y-1">
                {(selectedAgentData.capabilities || []).length === 0 ? (
                  <div className="text-center py-8 text-muted-foreground">
                    No capabilities found
                  </div>
                ) : (
                  (selectedAgentData.capabilities || []).map(
                    (capability: CapabilityInfo) => {
                      return (
                        <button
                          key={capability.id}
                          type="button"
                          onClick={() =>
                            handleCapabilitySelect(
                              selectedAgent!.id,
                              capability.id
                            )
                          }
                          className="w-full flex flex-col gap-1 px-3 py-3 rounded-lg text-left transition-colors hover:bg-muted"
                        >
                          <div className="flex items-center gap-2">
                            <span className="font-medium">
                              {capability.displayName || capability.name}
                            </span>
                          </div>
                          {capability.description && (
                            <p
                              className="text-xs text-muted-foreground line-clamp-2"
                              title={capability.description}
                            >
                              {capability.description}
                            </p>
                          )}
                        </button>
                      );
                    }
                  )
                )}
              </div>
            )
          ) : (
            // Browse View - Step Types + Operators
            <div className="space-y-6">
              {/* Step Types Section */}
              <div>
                <div className="flex items-center gap-2 mb-2">
                  <span className="text-lg">⚡</span>
                  <span className="text-xs font-semibold text-muted-foreground uppercase tracking-wide">
                    Step Types
                  </span>
                </div>
                <div className="space-y-1">
                  {filteredStepTypes.map((stepType) => (
                    <button
                      key={stepType.name}
                      type="button"
                      onClick={() => handleStepTypeSelect(stepType)}
                      className="w-full flex items-center gap-3 px-3 py-2 rounded-lg text-left transition-colors hover:bg-muted"
                    >
                      <StepTypeIcon
                        className="w-5 h-5 text-muted-foreground"
                        type={stepType.name || ''}
                      />
                      <div>
                        <div className="font-medium">{stepType.name}</div>
                        {stepType.description && (
                          <div
                            className="text-xs text-muted-foreground line-clamp-2"
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

              {/* Agents Section */}
              {filteredAgents.length > 0 && (
                <div>
                  <div className="flex items-center gap-2 mb-2">
                    <Bot className="w-5 h-5 text-muted-foreground" />
                    <span className="text-xs font-semibold text-muted-foreground uppercase tracking-wide">
                      {mode === 'memory' ? 'Memory Providers' : 'Agents'}
                    </span>
                  </div>
                  <div className="space-y-1">
                    {filteredAgents.map((agent) => {
                      const AgentIcon = getAgentIcon(agent.id);
                      return (
                        <button
                          key={agent.id}
                          type="button"
                          onClick={() =>
                            handleAgentSelect(agent.id || '', agent.name || '')
                          }
                          className="w-full flex items-center justify-between gap-2 px-3 py-2 rounded-lg text-left transition-colors hover:bg-muted"
                        >
                          <div className="flex items-center gap-3">
                            <div className="w-5 h-5 flex items-center justify-center shrink-0">
                              <AgentIcon className="w-5 h-5 text-muted-foreground" />
                            </div>
                            <div>
                              <div className="font-medium">{agent.name}</div>
                              {agent.description && (
                                <div
                                  className="text-xs text-muted-foreground line-clamp-2"
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
              )}
            </div>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
