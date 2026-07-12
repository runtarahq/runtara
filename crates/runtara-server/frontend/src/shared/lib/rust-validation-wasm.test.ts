import { beforeEach, describe, expect, it, vi } from 'vitest';

const wasm = vi.hoisted(() => ({
  init: vi.fn(async () => undefined),
  agentCatalogLoaded: vi.fn(() => false),
  initAgentCatalog: vi.fn(() => JSON.stringify({ success: true })),
  evaluateConditionJson: vi.fn(() =>
    JSON.stringify({ success: true, value: true })
  ),
}));

vi.mock('@/wasm/validation/runtara_validation.js', () => ({
  default: wasm.init,
  agentCatalogLoaded: wasm.agentCatalogLoaded,
  initAgentCatalog: wasm.initAgentCatalog,
  analyzeFormJson: vi.fn(),
  evaluateConditionJson: wasm.evaluateConditionJson,
  getAgentJson: vi.fn(),
  getAgentsJson: vi.fn(),
  getCapabilitySchemaJson: vi.fn(),
  getStepTypeSchemaJson: vi.fn(),
  getStepTypesJson: vi.fn(),
  normalizeSchemaFieldsFormJson: vi.fn(),
  validateExecutionGraphJson: vi.fn(),
  validateFormDefinitionJson: vi.fn(),
  validateSchemaFieldsJson: vi.fn(),
  validateWorkflowStartInputsJson: vi.fn(),
}));

vi.mock('@/wasm/validation/runtara_validation_bg.wasm?url', () => ({
  default: '/validation.wasm',
}));
vi.mock('@/shared/config/runtimeConfig', () => ({
  config: { oidc: { authority: '', clientId: '' } },
}));
vi.mock('@/shared/queries/utils', () => ({
  getRuntimeBaseUrl: () => 'http://runtime.test/api/runtime',
}));

describe('shared Rust validation initialization', () => {
  beforeEach(() => {
    vi.resetModules();
    vi.clearAllMocks();
    wasm.agentCatalogLoaded.mockReturnValue(false);
    wasm.initAgentCatalog.mockReturnValue(
      JSON.stringify({ success: true, agentCount: 2 })
    );
  });

  it('initializes domain-neutral validation without fetching workflow metadata', async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
    const { ensureRustValidationInitialized } = await import(
      './rust-validation-wasm'
    );

    await ensureRustValidationInitialized();

    expect(wasm.init).toHaveBeenCalledOnce();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('hydrates workflow validation from full agent detail payloads', async () => {
    const details = [
      { id: 'sftp', hasSideEffects: true, capabilities: [] },
      { id: 'http', hasSideEffects: true, capabilities: [] },
    ];
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(
        new Response(
          JSON.stringify({ agents: [{ id: 'sftp' }, { id: 'http' }] }),
          { status: 200 }
        )
      )
      .mockResolvedValueOnce(
        new Response(JSON.stringify(details[0]), { status: 200 })
      )
      .mockResolvedValueOnce(
        new Response(JSON.stringify(details[1]), { status: 200 })
      );
    vi.stubGlobal('fetch', fetchMock);
    const { ensureWorkflowValidationInitialized } = await import(
      './rust-validation-wasm'
    );

    await ensureWorkflowValidationInitialized();

    expect(fetchMock).toHaveBeenCalledTimes(3);
    expect(fetchMock).toHaveBeenNthCalledWith(
      2,
      'http://runtime.test/api/runtime/agents/sftp',
      expect.any(Object)
    );
    expect(wasm.initAgentCatalog).toHaveBeenCalledWith(JSON.stringify(details));
  });
});
