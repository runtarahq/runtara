import initRustValidation, {
  agentCatalogLoaded,
  getAgentJson,
  getAgentsJson,
  getCapabilitySchemaJson,
  getStepTypeSchemaJson,
  getStepTypesJson,
  initAgentCatalog,
  validateExecutionGraphJson,
  validateSchemaFieldsJson,
  validateWorkflowStartInputsJson,
} from '@/wasm/workflow-validation/runtara_workflow_validation.js';
import rustValidationWasmUrl from '@/wasm/workflow-validation/runtara_workflow_validation_bg.wasm?url';
import {
  AgentInfo,
  CapabilityInfo,
  ListStepTypesResponse,
} from '@/generated/RuntaraRuntimeApi';
import { RuntimeREST } from '@/shared/queries';

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

export interface RustSchemaFieldsValidationError {
  code: string;
  message: string;
  fieldName: string | null;
  rowIndices: number[];
}

export interface RustSchemaFieldsValidationResult
  extends RustWorkflowValidationResult {
  schemaErrors: RustSchemaFieldsValidationError[];
}

let initPromise: Promise<unknown> | null = null;

/**
 * One-shot fetch of `GET /api/runtime/agents` whose response is pushed
 * into the WASM via `initAgentCatalog`. The server returns the catalog
 * from its runtime `ComponentDispatcherService` (which loads each
 * `<agent>.meta.json` from `$RUNTARA_AGENT_COMPONENTS_DIR` at boot) —
 * so the validator + the runtime see the same agent set.
 *
 * Goes through `RuntimeREST` so the shared org_id-prefix interceptor in
 * `shared/queries/index.ts` rewrites the URL on multi-tenant deployments
 * — a raw `fetch` here would bypass it and 404 against the gateway.
 *
 * Idempotent: skips the fetch if `agentCatalogLoaded()` already returns
 * true (e.g. another caller initialized it).
 */
async function loadAgentCatalogIntoWasm(): Promise<void> {
  if (agentCatalogLoaded()) {
    return;
  }
  let body: { agents?: unknown };
  try {
    const response = await RuntimeREST.api.listAgentsHandler();
    body = response.data ?? { agents: [] };
  } catch (error) {
    throw new Error(
      `Failed to fetch /api/runtime/agents for validator init: ${
        error instanceof Error ? error.message : String(error)
      }`
    );
  }
  const agentsArray = Array.isArray(body?.agents) ? body.agents : [];
  const initResultRaw = initAgentCatalog(JSON.stringify(agentsArray));
  let parsed: { success?: boolean; agentCount?: number; error?: string };
  try {
    parsed = JSON.parse(initResultRaw) as typeof parsed;
  } catch {
    throw new Error(
      `Validator returned non-JSON from initAgentCatalog: ${initResultRaw}`
    );
  }
  if (!parsed.success) {
    throw new Error(
      `Validator rejected agent catalog payload: ${
        parsed.error ?? 'unknown error'
      }`
    );
  }
}

function ensureRustValidatorInitialized(): Promise<unknown> {
  initPromise ??= initRustValidation({
    module_or_path: rustValidationWasmUrl,
  })
    .then(() => loadAgentCatalogIntoWasm())
    .catch((error) => {
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
    status: valid ? 'valid' : 'invalid',
    errors: Array.isArray(response.errors) ? response.errors : [],
    warnings: Array.isArray(response.warnings) ? response.warnings : [],
    message:
      typeof response.message === 'string'
        ? response.message
        : 'Workflow validation completed',
    wasmAvailable: true,
  };
}

function normalizeSchemaFieldsValidationResponse(
  value: unknown
): RustSchemaFieldsValidationResult {
  const baseResult = normalizeValidationResponse(value);
  const response =
    value && typeof value === 'object'
      ? (value as { schemaErrors?: unknown })
      : {};
  const rawSchemaErrors = Array.isArray(response.schemaErrors)
    ? response.schemaErrors
    : [];

  return {
    ...baseResult,
    schemaErrors: rawSchemaErrors.map((rawError) => {
      const error =
        rawError && typeof rawError === 'object'
          ? (rawError as Partial<RustSchemaFieldsValidationError>)
          : {};

      return {
        code: typeof error.code === 'string' ? error.code : 'UNKNOWN',
        message:
          typeof error.message === 'string'
            ? error.message
            : 'Schema field validation failed',
        fieldName: typeof error.fieldName === 'string' ? error.fieldName : null,
        rowIndices: Array.isArray(error.rowIndices)
          ? error.rowIndices.filter(
              (rowIndex): rowIndex is number =>
                typeof rowIndex === 'number' && Number.isInteger(rowIndex)
            )
          : [],
      };
    }),
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

function unavailableSchemaFieldsValidationResult(
  error: unknown
): RustSchemaFieldsValidationResult {
  return {
    ...unavailableValidationResult(
      error,
      'Rust schema field validation unavailable; schema save cannot be validated'
    ),
    schemaErrors: [],
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
    console.warn(
      'Rust workflow start input validation WASM unavailable',
      error
    );
    return unavailableValidationResult(
      error,
      'Rust workflow start input validation unavailable; server validation remains active'
    );
  }
}

/**
 * Validate editable schema fields before converting them into map-based schema
 * JSON, where duplicate names would otherwise collapse.
 */
export async function validateSchemaFieldsWithRust(
  schemaLabel: string,
  fields: unknown[]
): Promise<RustSchemaFieldsValidationResult> {
  try {
    await ensureRustValidatorInitialized();

    const rawResult = validateSchemaFieldsJson(
      schemaLabel,
      JSON.stringify(fields ?? [])
    );

    return normalizeSchemaFieldsValidationResponse(JSON.parse(rawResult));
  } catch (error) {
    console.warn('Rust schema field validation WASM unavailable', error);
    return unavailableSchemaFieldsValidationResult(error);
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
