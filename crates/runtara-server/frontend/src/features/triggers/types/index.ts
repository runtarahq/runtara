// Triggers types - uses generated API types with UI enrichment
import {
  InvocationTrigger,
  TriggerType,
} from '@/generated/RuntaraRuntimeApi';

// Re-export generated types for convenience
export type { InvocationTrigger, TriggerType };

// Extended trigger type with UI enrichment from queries
// - scenarioName: resolved from scenario_id lookup
// - Converted from snake_case to camelCase for UI consistency
export interface EnrichedTrigger {
  id: string;
  scenarioId: string;
  scenarioName: string;
  triggerType: TriggerType;
  configuration?: object | null;
  configurationPreview?: string;
  active: boolean;
  singleInstance: boolean;
  remoteTenantId?: string | null;
  tenantId?: string | null;
  lastRun?: string | null;
  createdAt: string;
  updatedAt: string;
  /** Webhook URL for Channel triggers (computed by backend). */
  webhookUrl?: string | null;
}
