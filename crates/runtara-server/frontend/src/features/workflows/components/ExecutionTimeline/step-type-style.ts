import type { ElementType } from 'react';
import {
  Brain,
  Cpu,
  Database,
  Flag,
  GitBranch,
  Globe,
  Hand,
  Merge,
  MessageSquare,
  Repeat,
  Sparkles,
  Split,
  ToggleLeft,
  Wrench,
  Zap,
} from 'lucide-react';

// Categorical step-type IDENTITY colors (raw palette classes are intentional
// here — these are not status colors). text/bg/border/dot are Tailwind class
// equivalents of the previous hex + rgba inline styles.
export const stepTypeConfig: Record<
  string,
  {
    text: string;
    bg: string;
    border: string;
    dot: string;
    label: string;
    icon: ElementType;
  }
> = {
  Agent: {
    text: 'text-blue-500',
    bg: 'bg-blue-500/10',
    border: 'border-blue-500',
    dot: 'bg-blue-500',
    label: 'Agent',
    icon: Globe,
  },
  Connection: {
    text: 'text-violet-500',
    bg: 'bg-violet-500/10',
    border: 'border-violet-500',
    dot: 'bg-violet-500',
    label: 'Connection',
    icon: Zap,
  },
  Finish: {
    text: 'text-green-500',
    bg: 'bg-green-500/10',
    border: 'border-green-500',
    dot: 'bg-green-500',
    label: 'Finish',
    icon: Flag,
  },
  Conditional: {
    text: 'text-pink-500',
    bg: 'bg-pink-500/10',
    border: 'border-pink-500',
    dot: 'bg-pink-500',
    label: 'Conditional',
    icon: GitBranch,
  },
  While: {
    text: 'text-orange-500',
    bg: 'bg-orange-500/10',
    border: 'border-orange-500',
    dot: 'bg-orange-500',
    label: 'While',
    icon: Repeat,
  },
  Log: {
    text: 'text-indigo-500',
    bg: 'bg-indigo-500/10',
    border: 'border-indigo-500',
    dot: 'bg-indigo-500',
    label: 'Log',
    icon: MessageSquare,
  },
  Split: {
    text: 'text-teal-500',
    bg: 'bg-teal-500/10',
    border: 'border-teal-500',
    dot: 'bg-teal-500',
    label: 'Split',
    icon: Split,
  },
  Switch: {
    text: 'text-purple-500',
    bg: 'bg-purple-500/10',
    border: 'border-purple-500',
    dot: 'bg-purple-500',
    label: 'Switch',
    icon: ToggleLeft,
  },
  EmbedWorkflow: {
    text: 'text-amber-500',
    bg: 'bg-amber-500/10',
    border: 'border-amber-500',
    dot: 'bg-amber-500',
    label: 'Start Workflow',
    icon: Zap,
  },
  AiAgent: {
    text: 'text-violet-500',
    bg: 'bg-violet-500/10',
    border: 'border-violet-500',
    dot: 'bg-violet-500',
    label: 'AI Agent',
    icon: Sparkles,
  },
  AiAgentToolCall: {
    text: 'text-indigo-500',
    bg: 'bg-indigo-500/10',
    border: 'border-indigo-500',
    dot: 'bg-indigo-500',
    label: 'Tool Call',
    icon: Wrench,
  },
  AiAgentMemoryCompaction: {
    text: 'text-blue-500',
    bg: 'bg-blue-500/10',
    border: 'border-blue-500',
    dot: 'bg-blue-500',
    label: 'Memory Compaction',
    icon: Brain,
  },
  WaitForSignal: {
    text: 'text-amber-500',
    bg: 'bg-amber-500/10',
    border: 'border-amber-500',
    dot: 'bg-amber-500',
    label: 'Wait for Signal',
    icon: Hand,
  },
  WaitForInstances: {
    text: 'text-amber-500',
    bg: 'bg-amber-500/10',
    border: 'border-amber-500',
    dot: 'bg-amber-500',
    label: 'Wait for Instances',
    icon: Merge,
  },
  SetState: {
    text: 'text-cyan-500',
    bg: 'bg-cyan-500/10',
    border: 'border-cyan-500',
    dot: 'bg-cyan-500',
    label: 'Set State',
    icon: Database,
  },
  GetState: {
    text: 'text-cyan-500',
    bg: 'bg-cyan-500/10',
    border: 'border-cyan-500',
    dot: 'bg-cyan-500',
    label: 'Get State',
    icon: Database,
  },
  Default: {
    text: 'text-gray-500',
    bg: 'bg-gray-500/10',
    border: 'border-gray-500',
    dot: 'bg-gray-500',
    label: 'Step',
    icon: Cpu,
  },
};

/** Display label and swatch color for a step type, as the timeline draws it. */
export function getStepTypeStyle(type: string): { label: string; dot: string } {
  const config = stepTypeConfig[type];
  return config
    ? { label: config.label, dot: config.dot }
    : { label: type, dot: stepTypeConfig.Default.dot };
}
