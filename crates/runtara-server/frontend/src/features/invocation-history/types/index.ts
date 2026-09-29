/**
 * Type definitions for invocation history feature.
 */

import {
  ExecutionStatus,
  OperationErrorSummary,
  SuspensionReason,
  TerminationType,
} from '@/generated/RuntaraRuntimeApi';

/**
 * Extended execution instance with workflow name for display in history view.
 */
export interface ExecutionHistoryItem {
  instanceId: string;
  error?: string | null;
  errorSummary?: OperationErrorSummary | null;
  workflowId: string;
  workflowName?: string;
  runLabel?: string;
  createdAt: string;
  startedAt?: string | null;
  completedAt?: string | null;
  status: ExecutionStatus;
  terminationType?: TerminationType | null;
  version: number;
  executionDurationSeconds?: number | null;
  queueDurationSeconds?: number | null;
  maxMemoryMb?: number | null;
  tags?: string[];
  hasPendingInput?: boolean;
  /** Why a suspended run is not running; only `paused` needs a resume. */
  suspensionReason?: SuspensionReason | null;
  /** The run whose `control:start` step started this one. */
  parentInstanceId?: string | null;
}

/**
 * Filter options for the invocation history table.
 */
export interface ExecutionHistoryFilters {
  range?: '24h' | '7d' | 'all' | 'custom';
  dateBasis?: 'started' | 'completed';
  search?: string;
  runLabel?: string;
  /** Only the children of this run (started by its `control:start` steps). */
  parentInstanceId?: string;
  workflowId?: string;
  status?: string;
  createdFrom?: string;
  createdTo?: string;
  completedFrom?: string;
  completedTo?: string;
  sortBy?: 'createdAt' | 'completedAt' | 'status' | 'workflowId';
  sortOrder?: 'asc' | 'desc';
}
