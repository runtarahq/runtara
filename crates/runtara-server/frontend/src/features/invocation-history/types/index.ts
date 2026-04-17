/**
 * Type definitions for invocation history feature.
 */

import {
  ExecutionStatus,
  TerminationType,
} from '@/generated/RuntaraRuntimeApi';

/**
 * Extended execution instance with scenario name for display in history view.
 */
export interface ExecutionHistoryItem {
  instanceId: string;
  scenarioId: string;
  scenarioName?: string;
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
}

/**
 * Filter options for the invocation history table.
 */
export interface ExecutionHistoryFilters {
  scenarioId?: string;
  status?: string;
  createdFrom?: string;
  createdTo?: string;
  completedFrom?: string;
  completedTo?: string;
  sortBy?: 'createdAt' | 'completedAt' | 'status' | 'scenarioId';
  sortOrder?: 'asc' | 'desc';
}
