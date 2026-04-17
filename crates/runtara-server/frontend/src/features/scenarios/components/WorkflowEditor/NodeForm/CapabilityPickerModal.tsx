import { useState, useMemo, useContext, useEffect } from 'react';
import { ChevronLeft, ChevronRight, Search, Loader2 } from 'lucide-react';
import { Dialog, DialogContent } from '@/shared/components/ui/dialog';
import { Input } from '@/shared/components/ui/input';
import { cn } from '@/lib/utils';
import { useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { getAgentDetails, ExtendedAgent } from '@/features/scenarios/queries';
import { CapabilityInfo } from '@/generated/RuntaraRuntimeApi';
import { NodeFormContext } from './NodeFormContext';
import { useMultipleAgentDetails } from '@/features/scenarios/hooks';

interface CapabilitySearchResult {
  agentId: string;
  agentName: string;
  agentIcon: string;
  capability: CapabilityInfo;
  isSupported: boolean;
}

type ViewMode = 'browse' | 'search' | 'capabilities';

interface CapabilityPickerModalProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSelect: (agentId: string, capabilityId: string) => void;
  currentAgentId?: string;
  currentCapabilityId?: string;
}

/**
 * Get category icon emoji based on HDM category name
 */
function getCategoryIcon(category: string): string {
  const iconMap: Record<string, string> = {
    'E-Commerce': '📦',
    Commerce: '📦',
    CRM: '👥',
    ERP: '🏭',
    Analytics: '📊',
    Marketing: '📢',
    Other: '🔧',
  };
  return iconMap[category] || '🔧';
}

function getAgentCategory(_agent?: ExtendedAgent): string {
  void _agent;
  return 'Other';
}

/**
 * Modal dialog for selecting agent and capability with drilldown navigation
 * Supports global search across all agents and capabilities
 */
export function CapabilityPickerModal({
  open,
  onOpenChange,
  onSelect,
  currentAgentId,
}: CapabilityPickerModalProps) {
  const { agents } = useContext(NodeFormContext);
  const [viewMode, setViewMode] = useState<ViewMode>('browse');
  const [selectedAgent, setSelectedAgent] = useState<{
    id: string;
    name: string;
  } | null>(() => {
    if (!currentAgentId) return null;
    const ag = (agents as ExtendedAgent[])?.find(
      (a) => a.id === currentAgentId
    );
    return ag ? { id: ag.id, name: ag.name || '' } : null;
  });
  const [searchQuery, setSearchQuery] = useState('');

  // Get agent IDs for fetching details
  const agentIds = useMemo(
    () => ((agents || []) as ExtendedAgent[]).map((a) => a.id),
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

  // Fetch agent details when agent is selected (for capabilities view)
  const { data: agentDetails, isFetching } = useCustomQuery({
    queryKey: queryKeys.agents.byId(selectedAgent?.id ?? ''),
    queryFn: (token: string) => getAgentDetails(token, selectedAgent!.id),
    enabled: !!selectedAgent?.id && viewMode === 'capabilities',
  });

  // Reset state when modal closes
  const handleOpenChange = (newOpen: boolean) => {
    onOpenChange(newOpen);
    if (!newOpen) {
      setViewMode('browse');
      if (currentAgentId) {
        const ag = (agents as ExtendedAgent[])?.find(
          (a) => a.id === currentAgentId
        );
        setSelectedAgent(ag ? { id: ag.id, name: ag.name || '' } : null);
      } else {
        setSelectedAgent(null);
      }
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

  // Group agents by category
  const groupedAgents = useMemo(() => {
    const groups = new Map<string, ExtendedAgent[]>();

    for (const agent of (agents || []) as ExtendedAgent[]) {
      const category = getAgentCategory(agent);
      if (!groups.has(category)) {
        groups.set(category, []);
      }
      groups.get(category)!.push(agent);
    }

    const result: {
      category: string;
      icon: string;
      agents: ExtendedAgent[];
    }[] = [];
    const hdmCategories = Array.from(groups.keys()).filter(
      (k) => k !== 'Other'
    );

    hdmCategories.sort().forEach((category) => {
      result.push({
        category,
        icon: getCategoryIcon(category),
        agents: groups.get(category)!,
      });
    });

    const otherCategory = groups.get('Other');
    if (otherCategory) {
      result.push({
        category: 'Other',
        icon: getCategoryIcon('Other'),
        agents: otherCategory,
      });
    }

    return result;
  }, [agents]);

  // Global search across agents and capabilities
  const searchResults = useMemo(() => {
    if (!searchQuery.trim() || !allAgentsLoaded)
      return { agents: [], capabilities: [] };

    const query = searchQuery.toLowerCase();

    // Search agents
    const matchingAgents = ((agents || []) as ExtendedAgent[]).filter(
      (ag) =>
        ag.name?.toLowerCase().includes(query) ||
        ag.description?.toLowerCase().includes(query)
    );

    // Search capabilities across all agents
    const matchingCapabilities: CapabilitySearchResult[] = [];

    for (const agent of (agents || []) as ExtendedAgent[]) {
      const agentData = agentCapabilitiesMap.get(agent.id);
      if (!agentData) continue;

      const category = getAgentCategory(agent);

      for (const capability of agentData.capabilities) {
        const matchesSearch =
          capability.name?.toLowerCase().includes(query) ||
          capability.displayName?.toLowerCase().includes(query) ||
          capability.description?.toLowerCase().includes(query);

        if (matchesSearch) {
          matchingCapabilities.push({
            agentId: agent.id,
            agentName: agent.name || '',
            agentIcon: getCategoryIcon(category),
            capability,
            isSupported: true,
          });
        }
      }
    }

    return { agents: matchingAgents, capabilities: matchingCapabilities };
  }, [searchQuery, agents, agentCapabilitiesMap, allAgentsLoaded]);

  // Filter capabilities for selected agent
  const filteredCapabilities = useMemo(() => {
    const capabilities = agentDetails?.capabilities || [];
    return capabilities;
  }, [agentDetails?.capabilities]);

  const handleAgentSelect = (agentId: string, agentName: string) => {
    setSelectedAgent({ id: agentId, name: agentName });
    setViewMode('capabilities');
    setSearchQuery('');
  };

  const handleCapabilitySelect = (agentId: string, capabilityId: string) => {
    onSelect(agentId, capabilityId);
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
    return 'Select Capability';
  };

  const getSubtitle = () => {
    if (viewMode === 'search') return 'Matching agents and capabilities';
    if (viewMode === 'capabilities') return 'Choose a capability to perform';
    return 'Search or browse agents';
  };

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="sm:max-w-[500px] p-0 gap-0" hideCloseButton>
        {/* Header */}
        <div className="flex items-center gap-2 p-4 border-b">
          {viewMode === 'capabilities' && (
            <button
              type="button"
              onClick={handleBack}
              className="p-1 rounded hover:bg-muted"
            >
              <ChevronLeft className="h-5 w-5" />
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
              placeholder="Search agents or capabilities..."
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

              {searchResults.agents.length === 0 &&
              searchResults.capabilities.length === 0 ? (
                <div className="text-center py-8 text-muted-foreground">
                  No results found for "{searchQuery}"
                </div>
              ) : (
                <>
                  {/* Matching Agents */}
                  {searchResults.agents.length > 0 && (
                    <div>
                      <div className="text-xs font-semibold text-muted-foreground uppercase tracking-wide mb-2">
                        Agents
                      </div>
                      <div className="space-y-1">
                        {searchResults.agents.map((agent) => {
                          const category = getAgentCategory(agent);
                          return (
                            <button
                              key={agent.id}
                              type="button"
                              onClick={() =>
                                handleAgentSelect(agent.id, agent.name || '')
                              }
                              className="w-full flex items-center justify-between gap-2 px-3 py-2 rounded-lg text-left transition-colors hover:bg-muted"
                            >
                              <div className="flex items-center gap-3">
                                <span className="text-xl">
                                  {getCategoryIcon(category)}
                                </span>
                                <div>
                                  <div className="font-medium">
                                    {agent.name}
                                  </div>
                                  {agent.description && (
                                    <div className="text-xs text-muted-foreground line-clamp-1">
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
                              <span className="text-sm">
                                {result.agentIcon}
                              </span>
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
                              <p className="text-xs text-muted-foreground line-clamp-1 ml-6">
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
            isFetching ? (
              <div className="flex items-center justify-center py-8">
                <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
                <span className="ml-2 text-muted-foreground">
                  Loading capabilities...
                </span>
              </div>
            ) : (
              <div className="space-y-1">
                {filteredCapabilities.length === 0 ? (
                  <div className="text-center py-8 text-muted-foreground">
                    No capabilities found
                  </div>
                ) : (
                  filteredCapabilities.map((capability: CapabilityInfo) => {
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
                          <p className="text-xs text-muted-foreground line-clamp-2">
                            {capability.description}
                          </p>
                        )}
                      </button>
                    );
                  })
                )}
              </div>
            )
          ) : (
            // Browse Agents View
            <div className="space-y-4">
              {groupedAgents.length === 0 ? (
                <div className="text-center py-8 text-muted-foreground">
                  No agents available
                </div>
              ) : (
                groupedAgents.map((group) => (
                  <div key={group.category}>
                    <div className="flex items-center gap-2 mb-2">
                      <span className="text-lg">{group.icon}</span>
                      <span className="text-xs font-semibold text-muted-foreground uppercase tracking-wide">
                        {group.category}
                      </span>
                    </div>
                    <div className="space-y-1">
                      {group.agents.map((agent) => (
                        <button
                          key={agent.id}
                          type="button"
                          onClick={() =>
                            handleAgentSelect(agent.id, agent.name || '')
                          }
                          className={cn(
                            'w-full flex items-center justify-between gap-2 px-3 py-2 rounded-lg text-left transition-colors',
                            'hover:bg-muted',
                            selectedAgent?.id === agent.id && 'bg-muted'
                          )}
                        >
                          <div className="flex items-center gap-3">
                            <span className="text-xl">{group.icon}</span>
                            <div>
                              <div className="font-medium">{agent.name}</div>
                              {agent.description && (
                                <div className="text-xs text-muted-foreground line-clamp-1">
                                  {agent.description}
                                </div>
                              )}
                            </div>
                          </div>
                          <ChevronRight className="h-4 w-4 shrink-0 text-muted-foreground" />
                        </button>
                      ))}
                    </div>
                  </div>
                ))
              )}
            </div>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
