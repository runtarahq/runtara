import initRustValidation, {
  getAgentJson,
  getAgentsJson,
  getCapabilitySchemaJson,
  getStepTypeSchemaJson,
  getStepTypesJson,
  validateExecutionGraphJson,
  validateWorkflowStartInputsJson,
} from '@/wasm/workflow-validation/runtara_workflow_validation.js';
import rustValidationWasmUrl from '@/wasm/workflow-validation/runtara_workflow_validation_bg.wasm?url';
import {
  AgentInfo,
  CapabilityInfo,
  ListStepTypesResponse,
} from '@/generated/RuntaraRuntimeApi';

export interface RustWorkflowValidationResult {
  success: boolean;
  valid: boolean;
  status: 'valid' | 'invalid' | 'unavailable';
  errors: string[];
  warnings: string[];
  message: string;
  wasmAvailable: boolean;
  unavailableReason?: string;
}

let initPromise: Promise<unknown> | null = null;

function ensureRustValidatorInitialized(): Promise<unknown> {
  initPromise ??= initRustValidation({
    module_or_path: rustValidationWasmUrl,
  }).catch((error) => {
    initPromise = null;
    throw error;
  });
  return initPromise;
}

function normalizeValidationResponse(
  value: unknown
): RustWorkflowValidationResult {
  const response =
    value && typeof value === 'object'
      ? (value as Partial<RustWorkflowValidationResult>)
      : {};

  const success = response.success === true;
  const valid = success && response.valid === true;

  return {
    success,
    valid,
    status: success ? (valid ? 'valid' : 'invalid') : 'unavailable',
    errors: Array.isArray(response.errors) ? response.errors : [],
    warnings: Array.isArray(response.warnings) ? response.warnings : [],
    message:
      typeof response.message === 'string'
        ? response.message
        : 'Workflow validation completed',
    wasmAvailable: true,
  };
}

function unavailableValidationResult(
  error: unknown,
  message = 'Rust workflow validation unavailable; server validation remains active'
): RustWorkflowValidationResult {
  const unavailableReason =
    error instanceof Error ? error.message : String(error);

  return {
    success: false,
    valid: false,
    status: 'unavailable',
    errors: [],
    warnings: [],
    message,
    wasmAvailable: false,
    unavailableReason,
  };
}

/**
 * Validate the exact workflow start envelope sent to the backend using the
 * shared Rust input-schema validator compiled to WASM. If unavailable, return
 * an explicit unavailable state and let backend validation remain authoritative.
 */
export async function validateWorkflowStartInputsWithRust(
  inputSchema: unknown,
  inputs: unknown
): Promise<RustWorkflowValidationResult> {
  try {
    await ensureRustValidatorInitialized();

    const rawResult = validateWorkflowStartInputsJson(
      JSON.stringify(inputSchema ?? {}),
      JSON.stringify(inputs ?? {})
    );

    return normalizeValidationResponse(JSON.parse(rawResult));
  } catch (error) {
    console.warn('Rust workflow start input validation WASM unavailable', error);
    return unavailableValidationResult(
      error,
      'Rust workflow start input validation unavailable; server validation remains active'
    );
  }
}

function parseRustJson<T>(rawValue: string, fallback: T): T {
  const parsed = JSON.parse(rawValue);
  return parsed === null || parsed === undefined ? fallback : (parsed as T);
}

/**
 * Validate an execution graph in the browser using the Rust backend validator
 * compiled to WASM. If the browser cannot initialize or run the WASM module,
 * report validation as unavailable instead of valid. Save still relies on the
 * backend validator as the final source of truth.
 */
export async function validateExecutionGraphWithRust(
  executionGraph: unknown
): Promise<RustWorkflowValidationResult> {
  try {
    await ensureRustValidatorInitialized();

    const rawResult = validateExecutionGraphJson(
      JSON.stringify(executionGraph ?? {})
    );

    return normalizeValidationResponse(JSON.parse(rawResult));
  } catch (error) {
    console.warn('Rust workflow validation WASM unavailable', error);
    return unavailableValidationResult(error);
  }
}

export async function getStaticStepTypesWithRust(): Promise<ListStepTypesResponse> {
  await ensureRustValidatorInitialized();
  return parseRustJson<ListStepTypesResponse>(getStepTypesJson(), {
    step_types: [],
  });
}

export async function getStaticStepTypeSchemaWithRust(
  stepType: string
): Promise<unknown | null> {
  await ensureRustValidatorInitialized();
  return parseRustJson<unknown | null>(getStepTypeSchemaJson(stepType), null);
}

export async function getStaticAgentsWithRust(): Promise<AgentInfo[]> {
  await ensureRustValidatorInitialized();
  const response = parseRustJson<{ agents?: AgentInfo[] }>(getAgentsJson(), {
    agents: [],
  });
  return Array.isArray(response.agents) ? response.agents : [];
}

export async function getStaticAgentWithRust(
  agentId: string
): Promise<AgentInfo | null> {
  if (!agentId) {
    return null;
  }

  await ensureRustValidatorInitialized();
  return parseRustJson<AgentInfo | null>(getAgentJson(agentId), null);
}

export async function getStaticCapabilitySchemaWithRust(
  agentId: string,
  capabilityId: string
): Promise<CapabilityInfo | null> {
  if (!agentId || !capabilityId) {
    return null;
  }

  await ensureRustValidatorInitialized();
  return parseRustJson<CapabilityInfo | null>(
    getCapabilitySchemaJson(agentId, capabilityId),
    null
  );
}
