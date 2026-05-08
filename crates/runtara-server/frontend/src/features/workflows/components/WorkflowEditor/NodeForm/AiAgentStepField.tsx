import { useContext, useEffect, useMemo, useState } from 'react';
import { useFormContext, useWatch } from 'react-hook-form';
import {
  FormControl,
  FormItem,
  FormLabel,
  FormDescription,
} from '@/shared/components/ui/form';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { Input } from '@/shared/components/ui/input';
import { Button } from '@/shared/components/ui/button';
import {
  Plus,
  Trash2,
  ChevronDown,
  ChevronRight,
  Wrench,
  Brain,
  ListTree,
} from 'lucide-react';
import { NodeFormContext } from './NodeFormContext';
import {
  MappingValueInput,
  ValueMode,
} from './InputMappingField/MappingValueInput';
import { useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { getRuntimeBaseUrl } from '@/shared/queries/utils';
import { getConnections } from '@/features/connections/queries';
import { getPlatformIcon, getPlatformName } from '@/shared/utils/platform-info';
import { useWorkflowStore } from '@/features/workflows/stores/workflowStore';
import {
  SchemaFieldsEditor,
  type SchemaField as EditorSchemaField,
} from '../EditorSidebar/SchemaFieldsEditor';

const LLM_INTEGRATION_IDS = new Set(['openai_api_key', 'aws_credentials']);

type AiProvider = 'openai' | 'bedrock';

const PROVIDER_OPTIONS: Array<{
  value: AiProvider;
  label: string;
  integrationId: string;
}> = [
  { value: 'openai', label: 'OpenAI', integrationId: 'openai_api_key' },
  { value: 'bedrock', label: 'AWS Bedrock', integrationId: 'aws_credentials' },
];

interface ModelOption {
  value: string;
  label: string;
}

const OPENAI_MODELS: ModelOption[] = [
  { value: 'gpt-4.1', label: 'GPT-4.1' },
  { value: 'gpt-4.1-mini', label: 'GPT-4.1 Mini' },
  { value: 'gpt-4.1-nano', label: 'GPT-4.1 Nano' },
  { value: 'gpt-4o', label: 'GPT-4o' },
  { value: 'gpt-4o-mini', label: 'GPT-4o Mini' },
  { value: 'o3', label: 'o3' },
  { value: 'o3-mini', label: 'o3 Mini' },
  { value: 'o4-mini', label: 'o4 Mini' },
];

interface LlmModelMetadata {
  provider?: string;
  modelName?: string;
  modelId: string;
  recommendedForAiAgent?: boolean;
}

interface LlmModelsResponse {
  models?: LlmModelMetadata[];
}

async function getLlmModels(token: string | undefined, context?: any) {
  const provider = String(context?.queryKey?.[1] || 'bedrock');
  const response = await fetch(
    `${getRuntimeBaseUrl()}/metadata/llm-models?provider=${encodeURIComponent(
      provider
    )}`,
    {
      headers: token ? { Authorization: `Bearer ${token}` } : undefined,
    }
  );
  if (!response.ok) {
    throw new Error(`Failed to fetch LLM models: ${response.statusText}`);
  }
  const data = (await response.json()) as LlmModelsResponse;
  return (data.models ?? []).map((model) => ({
    value: model.modelId,
    label: model.modelName
      ? model.provider
        ? `${model.provider} ${model.modelName}`
        : model.modelName
      : model.modelId,
  }));
}

const RESERVED_HANDLE_IDS = new Set([
  'source',
  'target',
  'true',
  'false',
  'default',
  'onstart',
  'onError',
  'memory',
]);

const TOOL_NAME_REGEX = /^[a-zA-Z_][a-zA-Z0-9_]*$/;

type AiAgentStepFieldProps = {
  name: string;
};

export function AiAgentStepField({ name }: AiAgentStepFieldProps) {
  const form = useFormContext();
  const { nodeId, agents } = useContext(NodeFormContext);
  const stepType = useWatch({ name: 'stepType', control: form.control });
  const connectionId = useWatch({
    name: 'connectionId',
    control: form.control,
  });
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [showCompaction, setShowCompaction] = useState(false);
  const [newToolName, setNewToolName] = useState('');
  const [toolError, setToolError] = useState('');

  // Fetch all connections and filter to LLM types
  const connectionsQuery = useCustomQuery({
    queryKey: queryKeys.connections.all,
    queryFn: getConnections,
    placeholderData: [],
  });

  // Watch the inputMapping array to make fields reactive
  const inputMapping = useWatch({
    name,
    control: form.control,
    defaultValue: [],
  });

  const selectedConnectionIntegrationId = useMemo(() => {
    if (!connectionId) return null;
    const allConnections = connectionsQuery.data ?? [];
    const selected = allConnections.find(
      (conn: any) => conn.id === connectionId
    );
    return selected?.integrationId ?? null;
  }, [connectionId, connectionsQuery.data]);

  const selectedProvider: AiProvider | undefined = useMemo(() => {
    const providerField = (inputMapping || []).find(
      (item: any) => item.type === 'provider'
    )?.value;
    if (providerField === 'bedrock' || providerField === 'openai') {
      return providerField;
    }
    return undefined;
  }, [inputMapping]);

  const llmConnections = useMemo(() => {
    const allConnections = connectionsQuery.data ?? [];
    const expectedIntegrationId = PROVIDER_OPTIONS.find(
      (option) => option.value === selectedProvider
    )?.integrationId;
    if (!expectedIntegrationId) return [];
    return allConnections.filter(
      (conn: any) =>
        conn.integrationId &&
        LLM_INTEGRATION_IDS.has(conn.integrationId) &&
        conn.integrationId === expectedIntegrationId
    );
  }, [connectionsQuery.data, selectedProvider]);

  const connectionOptions = useMemo(() => {
    const noneOption = {
      label: 'None',
      value: '__none__',
      integrationId: null as string | null,
    };

    const options = llmConnections.map((conn: any) => ({
      label: conn.title || conn.id,
      value: conn.id,
      integrationId: conn.integrationId,
    }));

    return [noneOption, ...options];
  }, [llmConnections]);

  // Resolve selected connection's integrationId
  const selectedIntegrationId = useMemo(() => {
    if (!connectionId) return null;
    const selected = connectionOptions.find(
      (opt) => opt.value === connectionId
    );
    return selected?.integrationId ?? null;
  }, [connectionId, connectionOptions]);

  const bedrockModelsQuery = useCustomQuery<ModelOption[]>({
    queryKey: queryKeys.llmModels.byProvider('bedrock'),
    queryFn: getLlmModels,
    placeholderData: [],
    enabled: selectedProvider === 'bedrock',
  });

  const modelOptions = useMemo(() => {
    if (selectedProvider === 'bedrock') {
      return bedrockModelsQuery.data ?? [];
    }
    if (!selectedProvider) {
      return [];
    }
    return OPENAI_MODELS;
  }, [bedrockModelsQuery.data, selectedProvider]);

  // Get tool edges from workflow store
  const edges = useWorkflowStore((state) => state.edges);
  const nodes = useWorkflowStore((state) => state.nodes);
  const removeEdge = useWorkflowStore((state) => state.removeEdge);
  const removeNode = useWorkflowStore((state) => state.removeNode);

  const toolEdges = useMemo(() => {
    if (!nodeId) return [];
    return edges
      .filter(
        (edge) =>
          edge.source === nodeId &&
          edge.sourceHandle !== 'source' &&
          edge.sourceHandle !== 'memory' &&
          edge.sourceHandle
      )
      .map((edge) => {
        const targetNode = nodes.find((n) => n.id === edge.target);
        return {
          toolName: edge.sourceHandle!,
          targetId: edge.target,
          targetName: (targetNode?.data as any)?.name || edge.target,
          targetStepType: (targetNode?.data as any)?.stepType || '',
        };
      });
  }, [edges, nodes, nodeId]);

  // Initialize default fields for new AiAgent steps
  useEffect(() => {
    if (stepType !== 'AiAgent') return;
    if (nodeId) return; // Don't reset in edit mode

    const currentMapping = form.getValues(name) || [];
    if (currentMapping.length === 0) {
      form.setValue(name, [
        {
          type: 'provider',
          value: 'openai',
          typeHint: 'string',
          valueType: 'immediate',
        },
        {
          type: 'systemPrompt',
          value: '',
          typeHint: 'string',
          valueType: 'immediate',
        },
        {
          type: 'userPrompt',
          value: '',
          typeHint: 'string',
          valueType: 'immediate',
        },
        {
          type: 'model',
          value: '',
          typeHint: 'string',
          valueType: 'immediate',
        },
        {
          type: 'maxIterations',
          value: 10,
          typeHint: 'integer',
          valueType: 'immediate',
        },
        {
          type: 'temperature',
          value: 0.7,
          typeHint: 'number',
          valueType: 'immediate',
        },
      ]);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stepType, nodeId]);

  // Merge tool names from inputMapping with connected edges
  // NOTE: must be called before early return to satisfy Rules of Hooks
  const allToolNames = useMemo(() => {
    const fromMapping = Array.isArray(
      (inputMapping || []).find((item: any) => item.type === 'tools')?.value
    )
      ? (inputMapping.find((item: any) => item.type === 'tools')
          ?.value as string[])
      : [];
    const fromEdges = toolEdges.map((e) => e.toolName);
    // Union of both, preserving order from mapping first
    const seen = new Set(fromMapping);
    const merged = [...fromMapping];
    for (const edgeName of fromEdges) {
      if (!seen.has(edgeName)) {
        merged.push(edgeName);
        seen.add(edgeName);
      }
    }
    return merged;
  }, [inputMapping, toolEdges]);

  // Memory: list agents that have both memory:read and memory:write capability tags
  const memoryAgents = useMemo(() => {
    const result: Array<{ id: string; name: string; capabilityId: string }> =
      [];
    for (const agent of agents) {
      let hasRead = false;
      let hasWrite = false;
      let firstMemoryCap: string | null = null;
      for (const cap of Object.values(agent.supportedCapabilities)) {
        const tags = (cap as any).tags as string[] | undefined;
        if (tags?.includes('memory:read')) hasRead = true;
        if (tags?.includes('memory:write')) hasWrite = true;
        if (
          tags?.includes('memory:read') &&
          tags?.includes('memory:write') &&
          !firstMemoryCap
        ) {
          firstMemoryCap = cap.id;
        }
      }
      if (hasRead && hasWrite && firstMemoryCap) {
        result.push({
          id: agent.id,
          name: agent.name || agent.id,
          capabilityId: firstMemoryCap,
        });
      }
    }
    return result;
  }, [agents]);

  // Resolve which agent is currently selected as memory provider (by looking at the connected memory step's agentId)
  const currentMemoryAgentId = useMemo(() => {
    const mapping = inputMapping || [];
    const field = mapping.find(
      (item: any) => item.type === 'memoryProviderStepId'
    );
    const providerStepId = field?.value;
    if (!providerStepId) return null;
    const providerNode = nodes.find((n) => n.id === providerStepId);
    return (providerNode?.data as any)?.agentId || null;
  }, [nodes, inputMapping]);

  // Sync conversation_id from AI Agent → memory provider step whenever it changes
  useEffect(() => {
    if (stepType !== 'AiAgent') return;
    const mapping = inputMapping || [];
    const convField = mapping.find(
      (item: any) => item.type === 'memoryConversationId'
    );
    const providerField = mapping.find(
      (item: any) => item.type === 'memoryProviderStepId'
    );
    if (!convField || !providerField?.value) return;

    const providerStepId = providerField.value as string;
    const { nodes: storeNodes } = useWorkflowStore.getState();
    const providerNode = storeNodes.find((n) => n.id === providerStepId);
    if (!providerNode) return;

    const providerMapping = [
      ...((providerNode.data as any).inputMapping || []),
    ];
    const cidIdx = providerMapping.findIndex(
      (item: any) => item.type === 'conversation_id'
    );
    const newValue = convField.value;
    const newValueType = convField.valueType || 'reference';

    // Only update if the value actually changed
    if (cidIdx >= 0) {
      if (
        providerMapping[cidIdx].value === newValue &&
        providerMapping[cidIdx].valueType === newValueType
      )
        return;
      providerMapping[cidIdx] = {
        ...providerMapping[cidIdx],
        value: newValue,
        valueType: newValueType,
      };
    } else {
      providerMapping.push({
        type: 'conversation_id',
        value: newValue,
        valueType: newValueType,
        typeHint: 'string',
      });
    }

    const updatedNodes = storeNodes.map((n) => {
      if (n.id !== providerStepId) return n;
      return { ...n, data: { ...n.data, inputMapping: providerMapping } };
    });
    useWorkflowStore.setState({ nodes: updatedNodes, isDirty: true });
  }, [stepType, inputMapping]);

  if (stepType !== 'AiAgent') {
    return null;
  }

  // Helper to get current value from inputMapping array
  const getValue = (fieldName: string) => {
    const mapping = inputMapping || [];
    const field = mapping.find((item: any) => item.type === fieldName);
    return field?.value ?? '';
  };

  const getValueType = (fieldName: string): ValueMode => {
    const mapping = inputMapping || [];
    const field = mapping.find((item: any) => item.type === fieldName);
    return (field?.valueType as ValueMode) || 'immediate';
  };

  // Helper to update a field in the inputMapping array
  const updateField = (
    fieldName: string,
    value: any,
    valueType?: ValueMode
  ) => {
    const mapping = form.getValues(name) || [];
    const fieldIndex = mapping.findIndex(
      (item: any) => item.type === fieldName
    );

    if (fieldIndex >= 0) {
      form.setValue(`${name}.${fieldIndex}.value`, value, {
        shouldDirty: true,
        shouldTouch: true,
        shouldValidate: true,
      });
      if (valueType !== undefined) {
        form.setValue(`${name}.${fieldIndex}.valueType`, valueType, {
          shouldDirty: true,
          shouldTouch: true,
          shouldValidate: true,
        });
      }
    } else {
      form.setValue(
        name,
        [
          ...mapping,
          {
            type: fieldName,
            value,
            typeHint: 'string',
            valueType: valueType || 'immediate',
          },
        ],
        {
          shouldDirty: true,
          shouldTouch: true,
          shouldValidate: true,
        }
      );
    }
  };

  // Get current tools list from inputMapping
  const getToolNames = (): string[] => {
    const mapping = form.getValues(name) || [];
    const toolsEntry = mapping.find((item: any) => item.type === 'tools');
    return Array.isArray(toolsEntry?.value) ? toolsEntry.value : [];
  };

  const updateToolNames = (tools: string[]) => {
    const mapping = form.getValues(name) || [];
    const toolsIndex = mapping.findIndex((item: any) => item.type === 'tools');

    if (tools.length === 0) {
      // Remove tools entry if no tools
      if (toolsIndex >= 0) {
        const updated = [...mapping];
        updated.splice(toolsIndex, 1);
        form.setValue(name, updated, {
          shouldDirty: true,
          shouldTouch: true,
          shouldValidate: true,
        });
      }
      return;
    }

    if (toolsIndex >= 0) {
      // Update the full array at the parent level so useWatch({ name }) detects the change
      const updated = [...mapping];
      updated[toolsIndex] = { ...updated[toolsIndex], value: tools };
      form.setValue(name, updated, {
        shouldDirty: true,
        shouldTouch: true,
        shouldValidate: true,
      });
    } else {
      form.setValue(
        name,
        [
          ...mapping,
          {
            type: 'tools',
            value: tools,
            typeHint: 'json',
            valueType: 'immediate',
          },
        ],
        {
          shouldDirty: true,
          shouldTouch: true,
          shouldValidate: true,
        }
      );
    }
  };

  const handleAddTool = () => {
    const trimmed = newToolName.trim();
    if (!trimmed) {
      setToolError('Tool name is required');
      return;
    }
    if (!TOOL_NAME_REGEX.test(trimmed)) {
      setToolError(
        'Use only letters, numbers, underscores (start with letter or underscore)'
      );
      return;
    }
    if (RESERVED_HANDLE_IDS.has(trimmed)) {
      setToolError(`"${trimmed}" is a reserved name`);
      return;
    }
    const currentTools = getToolNames();
    if (currentTools.includes(trimmed)) {
      setToolError('Tool name already exists');
      return;
    }

    updateToolNames([...currentTools, trimmed]);
    setNewToolName('');
    setToolError('');
  };

  const handleRemoveTool = (toolName: string) => {
    const currentTools = getToolNames();
    updateToolNames(currentTools.filter((t) => t !== toolName));

    // Also remove the edge and orphaned target node if connected
    if (nodeId) {
      const toolEdge = edges.find(
        (e) => e.source === nodeId && e.sourceHandle === toolName
      );
      if (toolEdge) {
        const targetId = toolEdge.target;
        removeEdge(nodeId, targetId, toolName);

        // Remove the target node if it has no other incoming edges
        const remainingIncoming = edges.filter(
          (e) =>
            e.target === targetId &&
            !(e.source === nodeId && e.sourceHandle === toolName)
        );
        if (remainingIncoming.length === 0) {
          removeNode(targetId);
        }
      }
    }
  };

  return (
    <div className="space-y-4">
      {/* Provider Selector */}
      <FormItem>
        <FormLabel>Provider *</FormLabel>
        <FormDescription>
          Select the LLM provider for this agent
        </FormDescription>
        <Select
          value={selectedProvider}
          onValueChange={(value) => {
            const provider = value as AiProvider;
            updateField('provider', provider);
            updateField('model', '');
            const expectedIntegrationId = PROVIDER_OPTIONS.find(
              (option) => option.value === provider
            )?.integrationId;
            if (
              connectionId &&
              selectedConnectionIntegrationId &&
              selectedConnectionIntegrationId !== expectedIntegrationId
            ) {
              form.setValue('connectionId', '', {
                shouldDirty: true,
                shouldTouch: true,
                shouldValidate: true,
              });
            }
          }}
        >
          <FormControl>
            <SelectTrigger>
              <SelectValue placeholder="Select provider" />
            </SelectTrigger>
          </FormControl>
          <SelectContent>
            {PROVIDER_OPTIONS.map((provider) => (
              <SelectItem key={provider.value} value={provider.value}>
                {provider.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </FormItem>

      {/* Connection Selector */}
      <FormItem>
        <FormLabel>LLM Connection *</FormLabel>
        <FormDescription>
          {selectedProvider
            ? `Select a compatible ${
                selectedProvider === 'bedrock' ? 'AWS Bedrock' : 'OpenAI'
              } connection`
            : 'Select a provider first'}
        </FormDescription>
        <Select
          value={connectionId === '' ? '__none__' : connectionId || '__none__'}
          onValueChange={(value) => {
            form.setValue('connectionId', value === '__none__' ? '' : value, {
              shouldDirty: true,
              shouldTouch: true,
              shouldValidate: true,
            });
          }}
          disabled={!selectedProvider || connectionsQuery.isFetching}
        >
          <FormControl>
            <SelectTrigger>
              <SelectValue placeholder="Select LLM connection">
                {(() => {
                  const selected = connectionOptions.find(
                    (opt) =>
                      opt.value ===
                      (connectionId === ''
                        ? '__none__'
                        : connectionId || '__none__')
                  );
                  if (!selected) return 'Select LLM connection';
                  const icon = selected.integrationId
                    ? getPlatformIcon(selected.integrationId)
                    : null;
                  return (
                    <div className="flex items-center gap-1.5">
                      {icon && <span className="shrink-0 text-sm">{icon}</span>}
                      <span className="truncate">{selected.label}</span>
                    </div>
                  );
                })()}
              </SelectValue>
            </SelectTrigger>
          </FormControl>
          <SelectContent>
            {connectionOptions.length === 1 ? (
              <>
                <SelectItem value="__none__">None</SelectItem>
                <div className="px-2 py-3 text-sm text-muted-foreground text-center">
                  No LLM connections available. Create one in Connections.
                </div>
              </>
            ) : (
              connectionOptions.map((option) => {
                const platformName = getPlatformName(option.integrationId);
                const platformIcon = getPlatformIcon(option.integrationId);

                return (
                  <SelectItem key={option.value} value={option.value}>
                    <div className="flex items-center gap-2 min-w-0">
                      {option.integrationId && (
                        <span className="shrink-0 text-base">
                          {platformIcon}
                        </span>
                      )}
                      <div className="flex flex-col min-w-0 flex-1">
                        <span className="truncate">{option.label}</span>
                        {option.integrationId && (
                          <span className="text-xs text-muted-foreground truncate">
                            {platformName}
                          </span>
                        )}
                      </div>
                    </div>
                  </SelectItem>
                );
              })
            )}
          </SelectContent>
        </Select>
      </FormItem>

      {/* System Prompt */}
      <FormItem>
        <FormLabel>System Prompt *</FormLabel>
        <FormDescription>Instructions for the LLM agent</FormDescription>
        <FormControl>
          <MappingValueInput
            value={String(getValue('systemPrompt'))}
            onChange={(value) => updateField('systemPrompt', value)}
            valueType={getValueType('systemPrompt')}
            onValueTypeChange={(valueType) =>
              updateField('systemPrompt', getValue('systemPrompt'), valueType)
            }
            fieldType="textarea"
            fieldName="systemPrompt"
            placeholder="You are a helpful assistant..."
          />
        </FormControl>
      </FormItem>

      {/* User Prompt */}
      <FormItem>
        <FormLabel>User Prompt *</FormLabel>
        <FormDescription>
          The user message or request to process
        </FormDescription>
        <FormControl>
          <MappingValueInput
            value={String(getValue('userPrompt'))}
            onChange={(value) => updateField('userPrompt', value)}
            valueType={getValueType('userPrompt')}
            onValueTypeChange={(valueType) =>
              updateField('userPrompt', getValue('userPrompt'), valueType)
            }
            fieldType="textarea"
            fieldName="userPrompt"
            placeholder="Enter user prompt or select a reference..."
          />
        </FormControl>
      </FormItem>

      {/* Model */}
      <FormItem>
        <FormLabel>Model</FormLabel>
        <FormDescription>
          {!selectedProvider
            ? 'Select a provider first'
            : selectedIntegrationId
              ? 'Select a model or type a custom identifier'
              : 'Select a connection first to see available models'}
        </FormDescription>
        {modelOptions.length > 0 ? (
          <Select
            value={String(getValue('model') || '__none__')}
            onValueChange={(value) =>
              updateField('model', value === '__none__' ? '' : value)
            }
          >
            <FormControl>
              <SelectTrigger className="font-mono text-sm">
                <SelectValue placeholder="Select model...">
                  {getValue('model')
                    ? modelOptions.find((m) => m.value === getValue('model'))
                        ?.label || String(getValue('model'))
                    : 'Select model...'}
                </SelectValue>
              </SelectTrigger>
            </FormControl>
            <SelectContent>
              {modelOptions.map((model) => (
                <SelectItem key={model.value} value={model.value}>
                  <div className="flex flex-col">
                    <span>{model.label}</span>
                    <span className="text-xs text-muted-foreground font-mono">
                      {model.value}
                    </span>
                  </div>
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        ) : (
          <FormControl>
            <Input
              value={String(getValue('model'))}
              onChange={(e) => updateField('model', e.target.value)}
              placeholder="gpt-4o"
              className="font-mono text-sm"
            />
          </FormControl>
        )}
      </FormItem>

      {/* Advanced Settings */}
      <div className="border rounded-lg">
        <button
          type="button"
          className="flex items-center gap-2 w-full p-3 text-sm font-medium text-left hover:bg-muted/50 transition-colors"
          onClick={() => setShowAdvanced(!showAdvanced)}
        >
          {showAdvanced ? (
            <ChevronDown className="h-4 w-4" />
          ) : (
            <ChevronRight className="h-4 w-4" />
          )}
          Advanced Settings
        </button>
        {showAdvanced && (
          <div className="px-3 pb-3 space-y-4 border-t pt-3">
            {/* Max Iterations */}
            <FormItem>
              <FormLabel>Max Iterations</FormLabel>
              <FormDescription>
                Maximum tool-call loop iterations (1-50)
              </FormDescription>
              <FormControl>
                <Input
                  type="number"
                  min={1}
                  max={50}
                  value={getValue('maxIterations') ?? 10}
                  onChange={(e) =>
                    updateField(
                      'maxIterations',
                      e.target.value ? Number(e.target.value) : ''
                    )
                  }
                  className="w-24"
                />
              </FormControl>
            </FormItem>

            {/* Temperature */}
            <FormItem>
              <FormLabel>Temperature</FormLabel>
              <FormDescription>LLM sampling temperature (0-2)</FormDescription>
              <FormControl>
                <Input
                  type="number"
                  min={0}
                  max={2}
                  step={0.1}
                  value={getValue('temperature') ?? 0.7}
                  onChange={(e) =>
                    updateField(
                      'temperature',
                      e.target.value ? Number(e.target.value) : ''
                    )
                  }
                  className="w-24"
                />
              </FormControl>
            </FormItem>

            {/* Max Tokens */}
            <FormItem>
              <FormLabel>Max Tokens</FormLabel>
              <FormDescription>
                Maximum tokens per LLM response (leave empty for provider
                default)
              </FormDescription>
              <FormControl>
                <Input
                  type="number"
                  min={1}
                  value={getValue('maxTokens') || ''}
                  onChange={(e) =>
                    updateField(
                      'maxTokens',
                      e.target.value ? Number(e.target.value) : ''
                    )
                  }
                  placeholder="Provider default"
                  className="w-32"
                />
              </FormControl>
            </FormItem>
          </div>
        )}
      </div>

      {/* Tools Section */}
      <div className="border rounded-lg">
        <div className="flex items-center gap-2 p-3">
          <Wrench className="h-4 w-4 text-muted-foreground" />
          <span className="text-sm font-medium">Tools</span>
          <span className="text-xs text-muted-foreground">
            ({allToolNames.length})
          </span>
        </div>

        {allToolNames.length > 0 && (
          <div className="px-3 pb-2 space-y-1">
            {allToolNames.map((toolName) => {
              const edgeInfo = toolEdges.find((e) => e.toolName === toolName);
              return (
                <div
                  key={toolName}
                  className="flex items-center gap-2 px-2 py-1.5 rounded-md bg-muted/30 group"
                >
                  <span className="text-sm font-mono flex-1 truncate">
                    {toolName}
                  </span>
                  {edgeInfo && (
                    <span className="text-xs text-muted-foreground truncate max-w-[120px]">
                      {edgeInfo.targetName}
                    </span>
                  )}
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="h-6 w-6 opacity-0 group-hover:opacity-100 transition-opacity"
                    onClick={() => handleRemoveTool(toolName)}
                  >
                    <Trash2 className="h-3 w-3" />
                  </Button>
                </div>
              );
            })}
          </div>
        )}

        {/* Add tool input */}
        <div className="px-3 pb-3 pt-1">
          <div className="flex items-center gap-2">
            <Input
              value={newToolName}
              onChange={(e) => {
                setNewToolName(e.target.value);
                setToolError('');
              }}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  e.preventDefault();
                  handleAddTool();
                }
              }}
              placeholder="tool_name"
              className="font-mono text-sm h-8 flex-1"
            />
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="h-8"
              onClick={handleAddTool}
            >
              <Plus className="h-3 w-3 mr-1" />
              Add
            </Button>
          </div>
          {toolError && (
            <p className="text-xs text-destructive mt-1">{toolError}</p>
          )}
        </div>
      </div>

      <div className="rounded-md border border-violet-500/50 bg-violet-500/10 p-3 text-sm">
        <p className="text-violet-600 dark:text-violet-400">
          Tools are wired as labeled edges on the canvas. Add tool names here,
          then connect them to target steps (Agent, EmbedWorkflow, etc.) via the
          output handles on the node.
        </p>
      </div>

      {/* Output Schema Section */}
      <div className="border rounded-lg">
        <div className="flex items-center gap-2 p-3">
          <ListTree className="h-4 w-4 text-muted-foreground" />
          <span className="text-sm font-medium">Structured Output</span>
        </div>
        <div className="px-3 pb-3 space-y-2">
          <p className="text-xs text-muted-foreground">
            Define an output schema to force the LLM to return structured JSON
            matching this shape.
          </p>
          <SchemaFieldsEditor
            label="Output Schema"
            hideLabel
            showEnum
            fields={
              Array.isArray(getValue('outputSchema'))
                ? (getValue('outputSchema') as EditorSchemaField[])
                : []
            }
            onChange={(fields) =>
              updateField('outputSchema', fields, 'immediate')
            }
            emptyMessage="No output schema. The LLM will return free-form text."
          />
        </div>
      </div>

      {/* Memory Section — read-only display, memory is added/removed from the canvas */}
      {getValue('memoryEnabled') === true && (
        <div className="border rounded-lg">
          <div className="flex items-center justify-between p-3">
            <div className="flex items-center gap-2">
              <Brain className="h-4 w-4 text-muted-foreground" />
              <span className="text-sm font-medium">Conversation Memory</span>
            </div>
            <span className="text-xs text-muted-foreground">
              {(() => {
                const agent = memoryAgents.find(
                  (a) => a.id === currentMemoryAgentId
                );
                return agent ? agent.name : currentMemoryAgentId || 'Connected';
              })()}
            </span>
          </div>

          <div className="px-3 pb-3 space-y-4 border-t pt-3">
            {/* Conversation ID */}
            <FormItem>
              <FormLabel>Conversation ID *</FormLabel>
              <FormDescription>
                Messages with the same conversation ID share history across
                executions
              </FormDescription>
              <FormControl>
                <MappingValueInput
                  value={String(getValue('memoryConversationId'))}
                  onChange={(value) =>
                    updateField('memoryConversationId', value)
                  }
                  valueType={getValueType('memoryConversationId')}
                  onValueTypeChange={(valueType) =>
                    updateField(
                      'memoryConversationId',
                      getValue('memoryConversationId'),
                      valueType
                    )
                  }
                  fieldType="string"
                  fieldName="memoryConversationId"
                  placeholder="data.sessionId"
                />
              </FormControl>
            </FormItem>

            {/* Compaction (Advanced) */}
            <div className="border rounded-lg">
              <button
                type="button"
                className="flex items-center gap-2 w-full p-3 text-sm font-medium text-left hover:bg-muted/50 transition-colors"
                onClick={() => setShowCompaction(!showCompaction)}
              >
                {showCompaction ? (
                  <ChevronDown className="h-4 w-4" />
                ) : (
                  <ChevronRight className="h-4 w-4" />
                )}
                Compaction
              </button>
              {showCompaction && (
                <div className="px-3 pb-3 space-y-4 border-t pt-3">
                  <FormItem>
                    <FormLabel>Max Messages</FormLabel>
                    <FormDescription>
                      Trigger compaction when message count exceeds this
                      (1-1000)
                    </FormDescription>
                    <FormControl>
                      <Input
                        type="number"
                        min={1}
                        max={1000}
                        value={getValue('memoryMaxMessages') ?? 50}
                        onChange={(e) =>
                          updateField(
                            'memoryMaxMessages',
                            e.target.value ? Number(e.target.value) : ''
                          )
                        }
                        className="w-24"
                      />
                    </FormControl>
                  </FormItem>

                  <FormItem>
                    <FormLabel>Strategy</FormLabel>
                    <FormDescription>
                      How to compact old messages when the limit is reached
                    </FormDescription>
                    <Select
                      value={String(getValue('memoryStrategy') || 'summarize')}
                      onValueChange={(value) =>
                        updateField('memoryStrategy', value)
                      }
                    >
                      <FormControl>
                        <SelectTrigger>
                          <SelectValue />
                        </SelectTrigger>
                      </FormControl>
                      <SelectContent>
                        <SelectItem value="summarize">
                          <div className="flex flex-col">
                            <span>Summarize</span>
                            <span className="text-xs text-muted-foreground">
                              LLM summarizes old messages (costs 1 extra API
                              call)
                            </span>
                          </div>
                        </SelectItem>
                        <SelectItem value="slidingWindow">
                          <div className="flex flex-col">
                            <span>Sliding Window</span>
                            <span className="text-xs text-muted-foreground">
                              Drops oldest messages (free but loses context)
                            </span>
                          </div>
                        </SelectItem>
                      </SelectContent>
                    </Select>
                  </FormItem>
                </div>
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
