export const NODE_TYPES: Record<string, string> = {
  CreateNode: 'CREATE_NODE',
  BasicNode: 'BASIC_NODE',
  ConditionalNode: 'CONDITIONAL_NODE',
  SwitchNode: 'SWITCH_NODE',
  ContainerNode: 'CONTAINER_NODE',
  AiAgentNode: 'AI_AGENT_NODE',
  NoteNode: 'NOTE_NODE',
  StartIndicatorNode: 'START_INDICATOR_NODE',
};

// DSL v2.0.0 supported step types only
// Note: Start step type has been removed - entry point now points to first real step
export const STEP_TYPES: Record<string, string> = {
  Create: NODE_TYPES.CreateNode,
  Finish: NODE_TYPES.BasicNode,
  Agent: NODE_TYPES.BasicNode,
  Conditional: NODE_TYPES.ConditionalNode,
  Split: NODE_TYPES.ContainerNode,
  While: NODE_TYPES.ContainerNode,
  Switch: NODE_TYPES.SwitchNode,
  StartScenario: NODE_TYPES.BasicNode,
  'Start Scenario': NODE_TYPES.BasicNode, // Backend format (with space)
  // Legacy: Start step mapping kept for backward compatibility during migration
  Start: NODE_TYPES.BasicNode,
  Filter: NODE_TYPES.BasicNode,
  GroupBy: NODE_TYPES.BasicNode,
  AiAgent: NODE_TYPES.AiAgentNode,
  'AI Agent': NODE_TYPES.AiAgentNode, // Backend format (with space)
  WaitForSignal: NODE_TYPES.BasicNode,
  'Wait For Signal': NODE_TYPES.BasicNode, // Backend format (with space)
};

// All node sizes must be multiples of 12px grid for proper alignment
// Compact pill-shaped nodes for clean aesthetic
export const NODE_TYPE_SIZES: Record<
  string,
  { width: number; height: number }
> = {
  [NODE_TYPES.CreateNode]: { width: 132, height: 36 },
  [NODE_TYPES.BasicNode]: { width: 132, height: 36 },
  [NODE_TYPES.ConditionalNode]: { width: 132, height: 36 },
  [NODE_TYPES.SwitchNode]: { width: 132, height: 36 }, // Base size, dynamically adjusted based on cases
  [NODE_TYPES.AiAgentNode]: { width: 252, height: 96 }, // Card layout, dynamically adjusted based on tools/memory
  [NODE_TYPES.ContainerNode]: { width: 168, height: 132 },
  [NODE_TYPES.NoteNode]: { width: 192, height: 96 },
  [NODE_TYPES.StartIndicatorNode]: { width: 72, height: 36 },
};
