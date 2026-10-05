import type { ExecutionStatus } from '@/generated/RuntaraRuntimeApi';

export const RUN_STATUS_OPTIONS: { value: ExecutionStatus; label: string }[] = [
  { value: 'queued', label: 'Queued' },
  { value: 'compiling', label: 'Compiling' },
  { value: 'timeout', label: 'Timeout' },
  { value: 'running', label: 'Running' },
  { value: 'suspended', label: 'Waiting (including paused)' },
  { value: 'completed', label: 'Completed' },
  { value: 'failed', label: 'Failed' },
  { value: 'cancelled', label: 'Cancelled' },
];
