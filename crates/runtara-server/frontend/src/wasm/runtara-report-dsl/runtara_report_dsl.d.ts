/* tslint:disable */
/* eslint-disable */

/**
 * Evaluate a row condition (a `ConditionExpression` shape) against a row.
 * Returns true/false. Throws on server-only operators or malformed input.
 */
export function evaluateRowCondition(expr: any, row: any): boolean;

/**
 * One-shot value formatting outside a template. Same dispatch path as
 * the template filters, so `formatValue(x, 'currency', ctx)` matches
 * `renderTemplate('{{ x | currency }}', { x }, ctx)`.
 */
export function formatValue(value: any, format: string, ctx: any): string;

/**
 * Render a `{{ field | filter }}` template string against a row.
 * Throws on parse or render error. Locale-aware formatting routes back
 * into JS via the registered formatter callback.
 */
export function renderTemplate(template: string, row: any, ctx: any): string;

/**
 * Compile-check a template string. Returns `null` on success, throws on
 * parse error. Useful for save-time validation in the FE wizard.
 */
export function validateTemplate(template: string): void;

/**
 * Library version. Useful for FE↔BE drift detection.
 */
export function version(): string;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
  readonly memory: WebAssembly.Memory;
  readonly version: () => [number, number];
  readonly renderTemplate: (a: number, b: number, c: any, d: any) => [number, number, number, number];
  readonly formatValue: (a: any, b: number, c: number, d: any) => [number, number, number, number];
  readonly validateTemplate: (a: number, b: number) => [number, number];
  readonly evaluateRowCondition: (a: any, b: any) => [number, number, number];
  readonly __wbindgen_malloc: (a: number, b: number) => number;
  readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
  readonly __wbindgen_exn_store: (a: number) => void;
  readonly __externref_table_alloc: () => number;
  readonly __wbindgen_externrefs: WebAssembly.Table;
  readonly __wbindgen_free: (a: number, b: number, c: number) => void;
  readonly __externref_table_dealloc: (a: number) => void;
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
