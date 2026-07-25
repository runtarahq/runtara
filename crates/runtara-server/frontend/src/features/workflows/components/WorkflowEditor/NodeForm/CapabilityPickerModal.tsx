import { useState, useMemo, useContext, useEffect } from 'react';
import { ChevronLeft, ChevronRight, Search } from 'lucide-react';
import { Dialog, DialogContent } from '@/shared/components/ui/dialog';
import { Input } from '@/shared/components/ui/input';
import { cn } from '@/lib/utils';
import { useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { getAgentDetails, ExtendedAgent } from '@/features/workflows/queries';
import { CapabilityInfo } from '@/generated/RuntaraRuntimeApi';
import { NodeFormContext } from './NodeFormContext';
import { useMultipleAgentDetails } from '@/features/workflows/hooks';
import { useEntitlements } from '@/shared/hooks/useEntitlements';
import { agentEnabled } from '@/shared/entitlements';
import { SectionLabel } from '@/shared/components/section-label';
import {
  PICKER_DIALOG_WIDTH,
  PICKER_LIST_MAX_HEIGHT,
} from '@/shared/components/picker-dialog';
import { PickerEmpty } from '@/shared/components/picker-item';
import { Spinner } from '@/shared/components/ui/spinner';

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
  const { agents: rawAgents } = useContext(NodeFormContext);
  const entitlements = useEntitlements();

  // Hide disabled agents from the picker and avoid firing
  // `GET /api/runtime/agents/<module>` for them. Without this, the modal
  // opens and useMultipleAgentDetails fires a 403-bound request for every
  // disabled agent in the registry.
  const agents = useMemo(() => {
    const all = (rawAgents || []) as ExtendedAgent[];
    return all.filter((agent) => agentEnabled(entitlements, agent.id || ''));
  }, [rawAgents, entitlements]);

  const [viewMode, setViewMode] = useState<ViewMode>('browse');
  const [selectedAgent, setSelectedAgent] = useState<{
    id: string;
    name: string;
  } | null>(() => {
    if (!currentAgentId) return null;
    // Look up against the raw list — if the current step references a
    // now-disabled agent we still want to keep it visually selected (the
    // editor surfaces the stale state separately via the canvas badge).
    const ag = (rawAgents as ExtendedAgent[])?.find(
      (a) => a.id === currentAgentId
    );
    return ag ? { id: ag.id, name: ag.name || '' } : null;
  });
  const [searchQuery, setSearchQuery] = useState('');

  // Get agent IDs for fetching details — only for the filtered (enabled) list.
  const agentIds = useMemo(() => agents.map((a) => a.id), [agents]);

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
    // Also gate on entitlement so a stale-agent step (selected from the raw
    // list but disabled in the snapshot) doesn't fire a 403 here.
    enabled:
      !!selectedAgent?.id &&
      viewMode === 'capabilities' &&
      agentEnabled(entitlements, selectedAgent.id),
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
  const filteredCapabilities = useMemo(
    () => agentDetails?.capabilities || [],
    [agentDetails?.capabilities]
  );

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
      <DialogContent
        className={`gap-0 p-0 ${PICKER_DIALOG_WIDTH}`}
        hideCloseButton
      >
        {/* Header */}
        <div className="flex items-center gap-2 border-b p-4">
          {viewMode === 'capabilities' && (
            <button
              type="button"
              onClick={handleBack}
              className="rounded p-1 hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              <ChevronLeft className="size-5" />
            </button>
          )}
          <div className="flex-1">
            <h2 className="text-lg font-semibold">{getTitle()}</h2>
            <p className="text-sm text-muted-foreground">{getSubtitle()}</p>
          </div>
        </div>

        {/* Search */}
        <div className="border-b p-4">
          <div className="relative">
            <Search className="absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
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
        <div className={`${PICKER_LIST_MAX_HEIGHT} overflow-y-auto p-4`}>
          {viewMode === 'search' ? (
            // Search Results View
            <div className="space-y-4">
              {someLoading && (
                <div className="flex items-center gap-2 text-sm text-muted-foreground">
                  <Spinner className="size-4" />
                  Loading more results...
                </div>
              )}

              {searchResults.agents.length === 0 &&
              searchResults.capabilities.length === 0 ? (
                <PickerEmpty>No results found for "{searchQuery}"</PickerEmpty>
              ) : (
                <>
                  {/* Matching Agents */}
                  {searchResults.agents.length > 0 && (
                    <div>
                      <SectionLabel as="div" className="mb-2">
                        Agents
                      </SectionLabel>
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
                              className="flex w-full items-center justify-between gap-2 rounded-lg px-3 py-2 text-left transition-colors hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
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
                                    <div className="line-clamp-1 text-xs text-muted-foreground">
                                      {agent.description}
                                    </div>
                                  )}
                                </div>
                              </div>
                              <ChevronRight className="size-4 shrink-0 text-muted-foreground" />
                            </button>
                          );
                        })}
                      </div>
                    </div>
                  )}

                  {/* Matching Capabilities */}
                  {searchResults.capabilities.length > 0 && (
                    <div>
                      <SectionLabel as="div" className="mb-2">
                        Capabilities
                      </SectionLabel>
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
                              'flex w-full flex-col gap-1 rounded-lg px-3 py-3 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring',
                              result.isSupported
                                ? 'hover:bg-muted'
                                : 'cursor-not-allowed opacity-50'
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
                                <span className="rounded bg-warning/10 px-1.5 py-0.5 text-3xs font-medium text-warning">
                                  Coming Soon
                                </span>
                              )}
                            </div>
                            {result.capability.description && (
                              <p className="ml-6 line-clamp-1 text-xs text-muted-foreground">
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
                <Spinner className="size-6 text-muted-foreground" />
                <span className="ml-2 text-muted-foreground">
                  Loading capabilities...
                </span>
              </div>
            ) : (
              <div className="space-y-1">
                {filteredCapabilities.length === 0 ? (
                  <PickerEmpty>No capabilities found</PickerEmpty>
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
                        className="flex w-full flex-col gap-1 rounded-lg px-3 py-3 text-left transition-colors hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                      >
                        <div className="flex items-center gap-2">
                          <span className="font-medium">
                            {capability.displayName || capability.name}
                          </span>
                        </div>
                        {capability.description && (
                          <p className="line-clamp-2 text-xs text-muted-foreground">
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
                <PickerEmpty>No agents available</PickerEmpty>
              ) : (
                groupedAgents.map((group) => (
                  <div key={group.category}>
                    <div className="mb-2 flex items-center gap-2">
                      <span className="text-lg">{group.icon}</span>
                      <SectionLabel as="span">{group.category}</SectionLabel>
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
                            'flex w-full items-center justify-between gap-2 rounded-lg px-3 py-2 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring',
                            'hover:bg-muted',
                            selectedAgent?.id === agent.id && 'bg-muted'
                          )}
                        >
                          <div className="flex items-center gap-3">
                            <span className="text-xl">{group.icon}</span>
                            <div>
                              <div className="font-medium">{agent.name}</div>
                              {agent.description && (
                                <div className="line-clamp-1 text-xs text-muted-foreground">
                                  {agent.description}
                                </div>
                              )}
                            </div>
                          </div>
                          <ChevronRight className="size-4 shrink-0 text-muted-foreground" />
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
