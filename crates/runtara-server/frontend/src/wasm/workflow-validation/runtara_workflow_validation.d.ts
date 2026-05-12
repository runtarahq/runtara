/* tslint:disable */
/* eslint-disable */

/**
 * Return statically compiled metadata for one agent.
 */
export function getAgentJson(agent_id: string): string;

/**
 * Return statically compiled agent metadata, including capability schemas.
 */
export function getAgentsJson(): string;

/**
 * Return statically compiled capability metadata for one agent capability.
 */
export function getCapabilitySchemaJson(agent_id: string, capability_id: string): string;

/**
 * Return the JSON Schema metadata for a statically compiled workflow step type.
 */
export function getStepTypeSchemaJson(step_type: string): string;

/**
 * Return statically compiled workflow step type metadata.
 */
export function getStepTypesJson(): string;

/**
 * Validate an execution graph JSON string with the same Rust validation path
 * used by the backend.
 */
export function validateExecutionGraphJson(execution_graph_json: string): string;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
  readonly memory: WebAssembly.Memory;
  readonly validateExecutionGraphJson: (a: number, b: number) => [number, number];
  readonly getStepTypesJson: () => [number, number];
  readonly getStepTypeSchemaJson: (a: number, b: number) => [number, number];
  readonly getAgentsJson: () => [number, number];
  readonly getAgentJson: (a: number, b: number) => [number, number];
  readonly getCapabilitySchemaJson: (a: number, b: number, c: number, d: number) => [number, number];
  readonly __wbindgen_exn_store: (a: number) => void;
  readonly __externref_table_alloc: () => number;
  readonly __wbindgen_externrefs: WebAssembly.Table;
  readonly __wbindgen_malloc: (a: number, b: number) => number;
  readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
  readonly __wbindgen_free: (a: number, b: number, c: number) => void;
  readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
* Instantiates the given `module`, which can either be bytes or
* a precompiled `WebAssembly.Module`.
*
* @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
*
* @returns {InitOutput}
*/
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
* If `module_or_path` is {RequestInfo} or {URL}, makes a request and
* for everything else, calls `WebAssembly.instantiate` directly.
*
* @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
*
* @returns {Promise<InitOutput>}
*/
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
