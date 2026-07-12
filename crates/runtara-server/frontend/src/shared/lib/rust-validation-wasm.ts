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
let agentCatalogPromise: Promise<void> | null = null;

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
  const summaries = Array.isArray(body?.agents) ? body.agents : [];
  const agents = await Promise.all(
    summaries.map(async (summary) => {
      const id =
        summary && typeof summary === 'object' && 'id' in summary
          ? (summary as { id?: unknown }).id
          : undefined;
      if (typeof id !== 'string' || id.length === 0) {
        throw new Error('Agent catalog summary is missing a valid id');
      }
      const detailUrl = `${url}/${encodeURIComponent(id)}`;
      const detailResponse = await fetch(detailUrl, {
        credentials: 'include',
        headers,
      });
      if (!detailResponse.ok) {
        throw new Error(
          `${detailUrl} returned HTTP ${detailResponse.status} ${detailResponse.statusText}`
        );
      }
      return detailResponse.json();
    })
  );
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

/** Initialize the domain-neutral Rust validation bundle. */
export function ensureRustValidationInitialized(): Promise<unknown> {
  initPromise ??= initRustValidation({
    module_or_path: rustValidationWasmUrl,
  }).catch((error) => {
    initPromise = null;
    throw error;
  });
  return initPromise;
}

/** Initialize workflow metadata after the shared validator is available. */
export async function ensureWorkflowValidationInitialized(): Promise<void> {
  await ensureRustValidationInitialized();
  agentCatalogPromise ??= loadAgentCatalogIntoWasm().catch((error) => {
    agentCatalogPromise = null;
    throw error;
  });
  return agentCatalogPromise;
}
