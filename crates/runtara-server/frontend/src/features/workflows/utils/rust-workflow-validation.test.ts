import { readFileSync } from 'node:fs';
import path from 'node:path';
import {
  afterAll,
  beforeAll,
  beforeEach,
  describe,
  expect,
  it,
  vi,
} from 'vitest';
import {
  getStaticAgentWithRust,
  getStaticAgentsWithRust,
  getStaticCapabilitySchemaWithRust,
  getStaticStepTypeSchemaWithRust,
  getStaticStepTypesWithRust,
  validateExecutionGraphWithRust,
  validateSchemaFieldsWithRust,
  validateWorkflowStartInputsWithRust,
} from './rust-workflow-validation';

const wasmBytes = readFileSync(
  path.resolve(
    process.cwd(),
    'src/wasm/workflow-validation/runtara_workflow_validation_bg.wasm'
  )
);
const originalFetch = globalThis.fetch.bind(globalThis);
let agentsFetchCount = 0;

/**
 * Sample catalog returned by the stubbed `/api/runtime/agents`. Shape
 * matches the real `ListAgentsResponse` shape so the validator unwraps
 * `.agents` correctly and pushes the array into the WASM.
 */
const TEST_AGENT_CATALOG = {
  agents: [
    {
      id: 'http',
      name: 'HTTP',
      description: 'HTTP requests',
      hasSideEffects: true,
      supportsConnections: true,
      integrationIds: ['http_bearer', 'http_basic'],
      capabilities: [
        {
          id: 'http-request',
          name: 'HTTP Request',
          inputType: 'HttpRequestInput',
          inputs: [{ name: 'url', type: 'string', required: true }],
          output: { type: 'HttpResponse' },
          hasSideEffects: true,
          isIdempotent: false,
          rateLimited: false,
        },
      ],
    },
  ],
};

function stubWasmFetch() {
  vi.stubGlobal(
    'fetch',
    async (input: RequestInfo | URL, init?: RequestInit) => {
      const target =
        typeof input === 'string'
          ? input
          : input instanceof URL
            ? input.href
            : input.url;

      if (target.endsWith('runtara_workflow_validation_bg.wasm')) {
        return new Response(wasmBytes, {
          headers: { 'Content-Type': 'application/wasm' },
        });
      }

      if (target.endsWith('/api/runtime/agents')) {
        agentsFetchCount += 1;
        return new Response(JSON.stringify(TEST_AGENT_CATALOG), {
          headers: { 'Content-Type': 'application/json' },
        });
      }

      return originalFetch(input, init);
    }
  );
}

describe('rust workflow validation WASM', () => {
  beforeAll(() => {
    stubWasmFetch();
  });

  beforeEach(() => {
    agentsFetchCount = 0;
  });

  afterAll(() => {
    vi.stubGlobal('fetch', originalFetch);
  });

  it('reports unavailable instead of valid when WASM initialization fails', async () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    vi.stubGlobal('fetch', async () => {
      throw new Error('forced WASM init failure');
    });

    try {
      const result = await validateExecutionGraphWithRust({});

      expect(result.wasmAvailable).toBe(false);
      expect(result.success).toBe(false);
      expect(result.valid).toBe(false);
      expect(result.status).toBe('unavailable');
      expect(result.errors).toEqual([]);
      expect(result.message).toContain('unavailable');
    } finally {
      warnSpy.mockRestore();
      stubWasmFetch();
    }
  });

  it('initializes generated WASM and validates execution graphs', async () => {
    const result = await validateExecutionGraphWithRust({});

    expect(result.wasmAvailable).toBe(true);
    expect(result.success).toBe(true);
    expect(result.valid).toBe(false);
    expect(result.status).toBe('invalid');
    expect(result.errors.length).toBeGreaterThan(0);
    expect(agentsFetchCount).toBe(1);
  });

  it('reports Rust graph parse failures as invalid, not unavailable', async () => {
    const result = await validateExecutionGraphWithRust([]);

    expect(result.wasmAvailable).toBe(true);
    expect(result.status).toBe('invalid');
    expect(result.errors.join(' ')).toContain('graph must be a JSON object');
  });

  it('validates workflow start inputs with generated WASM', async () => {
    const result = await validateWorkflowStartInputsWithRust(
      { count: { type: 'integer', required: true } },
      { data: { count: 3 }, variables: {} }
    );

    expect(result.wasmAvailable).toBe(true);
    expect(result.success).toBe(true);
    expect(result.valid).toBe(true);
    expect(result.status).toBe('valid');
    expect(result.errors).toEqual([]);
  });

  it('validates editable schema fields with generated WASM', async () => {
    const result = await validateSchemaFieldsWithRust('Input schema', [
      { name: 'order_id', type: 'string', required: true },
      { name: ' order_id ', type: 'number', required: false },
      { name: 'customer_id', type: 'string', required: false },
    ]);

    expect(result.wasmAvailable).toBe(true);
    expect(result.success).toBe(true);
    expect(result.valid).toBe(false);
    expect(result.status).toBe('invalid');
    expect(result.errors).toEqual([
      "[E008] Input schema field name 'order_id' is duplicated. Field names must be unique.",
    ]);
    expect(result.schemaErrors).toEqual([
      {
        code: 'E008',
        message:
          "[E008] Input schema field name 'order_id' is duplicated. Field names must be unique.",
        fieldName: 'order_id',
        rowIndices: [0, 1],
      },
    ]);
  });

  it('rejects workflow start inputs with backend-equivalent generated WASM', async () => {
    const result = await validateWorkflowStartInputsWithRust(
      { count: { type: 'integer', required: true } },
      { data: { count: 'not-a-number' }, variables: {} }
    );

    expect(result.wasmAvailable).toBe(true);
    expect(result.success).toBe(true);
    expect(result.valid).toBe(false);
    expect(result.status).toBe('invalid');
    expect(result.errors.join(' ')).toContain('count');
  });

  it('serves step types from WASM and agents from the runtime API catalog', async () => {
    const stepTypes = await getStaticStepTypesWithRust();
    const agentStepSchema = await getStaticStepTypeSchemaWithRust('Agent');
    const agents = await getStaticAgentsWithRust();

    expect(stepTypes.step_types).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ id: 'Start', name: 'Start' }),
        expect.objectContaining({ id: 'Agent', name: 'Agent' }),
      ])
    );
    expect(agentStepSchema).toEqual(
      expect.objectContaining({
        type: 'Agent',
        displayName: 'Agent',
      })
    );
    expect(agents.length).toBeGreaterThan(0);

    const firstAgent = agents.find((agent) => agent.capabilities.length > 0);
    expect(firstAgent).toBeDefined();

    const agent = await getStaticAgentWithRust(firstAgent!.id);
    expect(agent).toEqual(expect.objectContaining({ id: firstAgent!.id }));

    const capability = firstAgent!.capabilities[0];
    const capabilitySchema = await getStaticCapabilitySchemaWithRust(
      firstAgent!.id,
      capability.id
    );
    expect(capabilitySchema).toEqual(
      expect.objectContaining({
        id: capability.id,
        inputs: expect.any(Array),
      })
    );
  });
});
