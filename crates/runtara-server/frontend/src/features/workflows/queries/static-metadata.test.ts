import { readFileSync } from 'node:fs';
import path from 'node:path';
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest';

const runtimeApiMock = vi.hoisted(() => ({
  listAgentsHandler: vi.fn(() => {
    throw new Error('listAgentsHandler should not be called for static metadata');
  }),
  listStepTypesHandler: vi.fn(() => {
    throw new Error(
      'listStepTypesHandler should not be called for static metadata'
    );
  }),
}));

vi.mock('@/shared/queries', () => ({
  RuntimeREST: {
    api: runtimeApiMock,
  },
}));

import {
  getAgentDetails,
  getAgents,
  getWorkflowStepTypes,
} from '@/features/workflows/queries';

const wasmBytes = readFileSync(
  path.resolve(
    process.cwd(),
    'src/wasm/workflow-validation/runtara_workflow_validation_bg.wasm'
  )
);
const originalFetch = globalThis.fetch.bind(globalThis);

describe('workflow static metadata queries', () => {
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

  it('loads agent and step metadata from WASM without backend metadata calls', async () => {
    const stepTypes = await getWorkflowStepTypes('token');
    const agentsResponse = await getAgents('token');
    const firstAgent = agentsResponse.agents[0];
    const agentDetails = await getAgentDetails('token', firstAgent.id);

    expect(stepTypes.step_types.length).toBeGreaterThan(0);
    expect(agentsResponse.agents.length).toBeGreaterThan(0);
    expect(firstAgent.supportedCapabilities).not.toEqual({});
    expect(
      agentsResponse.agents.find((agent) => agent.id === 'compression')
        ?.supportedCapabilities['create-archive']?.inputs
    ).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          name: 'files',
          required: true,
        }),
      ])
    );
    expect(agentDetails).toEqual(
      expect.objectContaining({
        id: firstAgent.id,
        capabilities: expect.any(Array),
      })
    );
    expect(agentDetails?.capabilities.length).toBeGreaterThan(0);
    expect(runtimeApiMock.listAgentsHandler).not.toHaveBeenCalled();
    expect(runtimeApiMock.listStepTypesHandler).not.toHaveBeenCalled();
  });
});
