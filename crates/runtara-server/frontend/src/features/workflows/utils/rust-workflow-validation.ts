import initRustValidation, {
  getAgentJson,
  getAgentsJson,
  getCapabilitySchemaJson,
  getStepTypeSchemaJson,
  getStepTypesJson,
  validateExecutionGraphJson,
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
  errors: string[];
  warnings: string[];
  message: string;
  wasmAvailable: boolean;
}

let initPromise: Promise<unknown> | null = null;

function ensureRustValidatorInitialized(): Promise<unknown> {
  initPromise ??= initRustValidation({
    module_or_path: rustValidationWasmUrl,
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

  return {
    success: response.success === true,
    valid: response.valid === true,
    errors: Array.isArray(response.errors) ? response.errors : [],
    warnings: Array.isArray(response.warnings) ? response.warnings : [],
    message:
      typeof response.message === 'string'
        ? response.message
        : 'Workflow validation completed',
    wasmAvailable: true,
  };
}

function parseRustJson<T>(rawValue: string, fallback: T): T {
  const parsed = JSON.parse(rawValue);
  return parsed === null || parsed === undefined ? fallback : (parsed as T);
}

/**
 * Validate an execution graph in the browser using the Rust backend validator
 * compiled to WASM. If the browser cannot initialize the WASM module, this
 * deliberately returns a non-blocking result because the server still runs the
 * same validation on save.
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
    return {
      success: false,
      valid: true,
      errors: [],
      warnings: [],
      message:
        'Rust workflow validation unavailable; server validation remains active',
      wasmAvailable: false,
    };
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
