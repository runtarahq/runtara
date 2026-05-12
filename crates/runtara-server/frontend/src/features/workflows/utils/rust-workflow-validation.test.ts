import { readFileSync } from 'node:fs';
import path from 'node:path';
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest';
import {
  getStaticAgentWithRust,
  getStaticAgentsWithRust,
  getStaticCapabilitySchemaWithRust,
  getStaticStepTypeSchemaWithRust,
  getStaticStepTypesWithRust,
  validateExecutionGraphWithRust,
} from './rust-workflow-validation';

const wasmBytes = readFileSync(
  path.resolve(
    process.cwd(),
    'src/wasm/workflow-validation/runtara_workflow_validation_bg.wasm'
  )
);
const originalFetch = globalThis.fetch.bind(globalThis);

describe('rust workflow validation WASM', () => {
  beforeAll(() => {
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

        return originalFetch(input, init);
      }
    );
  });

  afterAll(() => {
    vi.stubGlobal('fetch', originalFetch);
  });

  it('initializes generated WASM and validates execution graphs', async () => {
    const result = await validateExecutionGraphWithRust({});

    expect(result.wasmAvailable).toBe(true);
    expect(result.success).toBe(true);
    expect(result.valid).toBe(false);
    expect(result.errors.length).toBeGreaterThan(0);
  });

  it('returns statically compiled workflow metadata from generated WASM', async () => {
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
