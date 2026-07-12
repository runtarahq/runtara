import { User } from 'oidc-client-ts';

import initRustValidation, {
  agentCatalogLoaded,
  analyzeFormJson,
  getAgentJson,
  getAgentsJson,
  getCapabilitySchemaJson,
  getStepTypeSchemaJson,
  getStepTypesJson,
  initAgentCatalog,
  validateExecutionGraphJson,
  validateFormDefinitionJson,
  validateSchemaFieldsJson,
  validateWorkflowStartInputsJson,
} from '@/wasm/workflow-validation/runtara_workflow_validation.js';
import rustValidationWasmUrl from '@/wasm/workflow-validation/runtara_workflow_validation_bg.wasm?url';
import { config } from '@/shared/config/runtimeConfig';
import { getRuntimeBaseUrl } from '@/shared/queries/utils';

export {
  analyzeFormJson,
  getAgentJson,
  getAgentsJson,
  getCapabilitySchemaJson,
  getStepTypeSchemaJson,
  getStepTypesJson,
  validateExecutionGraphJson,
  validateFormDefinitionJson,
  validateSchemaFieldsJson,
  validateWorkflowStartInputsJson,
};

let initPromise: Promise<unknown> | null = null;

function readAccessTokenFromStorage(): string | undefined {
  const authority = config.oidc.authority;
  const clientId = config.oidc.clientId;
  if (!authority || !clientId) {
    return undefined;
  }
  if (typeof window === 'undefined' || !window.localStorage) {
    return undefined;
  }
  const raw = window.localStorage.getItem(`oidc.user:${authority}:${clientId}`);
  if (!raw) {
    return undefined;
  }
  try {
    const user = User.fromStorageString(raw);
    return user.expired ? undefined : user.access_token;
  } catch {
    return undefined;
  }
}

async function loadAgentCatalogIntoWasm(): Promise<void> {
  if (agentCatalogLoaded()) {
    return;
  }
  const url = `${getRuntimeBaseUrl()}/agents`;
  const token = readAccessTokenFromStorage();
  const headers: Record<string, string> = { accept: 'application/json' };
  if (token) {
    headers.Authorization = `Bearer ${token}`;
  }
  const response = await fetch(url, {
    credentials: 'include',
    headers,
  });
  if (!response.ok) {
    throw new Error(
      `${url} returned HTTP ${response.status} ${response.statusText}`
    );
  }
  const body = (await response.json()) as { agents?: unknown };
  const agents = Array.isArray(body?.agents) ? body.agents : [];
  const result = JSON.parse(initAgentCatalog(JSON.stringify(agents))) as {
    success?: boolean;
    error?: string;
  };
  if (!result.success) {
    throw new Error(
      `Validator rejected agent catalog payload: ${result.error ?? 'unknown error'}`
    );
  }
}

/** Initialize the single shared Rust validation bundle for every UI domain. */
export function ensureRustValidationInitialized(): Promise<unknown> {
  initPromise ??= initRustValidation({ module_or_path: rustValidationWasmUrl })
    .then(() => loadAgentCatalogIntoWasm())
    .catch((error) => {
      initPromise = null;
      throw error;
    });
  return initPromise;
}
