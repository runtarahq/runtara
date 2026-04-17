// ---------------------------------------------------------------------------
// Payload base and variants
// ---------------------------------------------------------------------------

export interface StepDebugPayloadBase {
  step_id: string;
  step_name?: string;
  step_type: string;
  scope_id?: string | null;
  parent_scope_id?: string | null;
  loop_indices: unknown[];
  timestamp_ms: number;
}

export interface StepDebugStartPayload extends StepDebugPayloadBase {
  inputs: unknown;
  input_mapping?: Record<string, unknown>;
}

export interface StepDebugEndPayload extends StepDebugPayloadBase {
  outputs: unknown;
  duration_ms: number;
}

export interface WorkflowLogPayload {
  step_name?: string;
  step_id?: string;
  level?: string;
  message?: string;
  context_data?: Record<string, unknown>;
}

// ---------------------------------------------------------------------------
// Type guards
// ---------------------------------------------------------------------------

export function isStepDebugStartPayload(
  payload: unknown
): payload is StepDebugStartPayload {
  if (typeof payload !== 'object' || payload === null) return false;
  const p = payload as Record<string, unknown>;
  return typeof p.step_id === 'string' && 'inputs' in p;
}

export function isStepDebugEndPayload(
  payload: unknown
): payload is StepDebugEndPayload {
  if (typeof payload !== 'object' || payload === null) return false;
  const p = payload as Record<string, unknown>;
  return typeof p.step_id === 'string' && 'outputs' in p;
}

export function isWorkflowLogPayload(
  payload: unknown
): payload is WorkflowLogPayload {
  if (typeof payload !== 'object' || payload === null) return false;
  const p = payload as Record<string, unknown>;
  return 'message' in p || 'level' in p;
}
