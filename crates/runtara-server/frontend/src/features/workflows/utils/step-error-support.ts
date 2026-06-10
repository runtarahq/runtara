/**
 * Step types the compiler supports `onError` routing on.
 *
 * Mirrors `on_error_route_shape_supported` in
 * `crates/runtara-workflows/src/direct_wasm/support.rs`: Agent, EmbedWorkflow,
 * Split, While, AiAgent and WaitForSignal steps may carry `onError` edges;
 * every other step type is rejected by the direct compiler, so the editor must
 * not offer an error route there. The backend aliases with spaces are included
 * because loaded workflows may carry the display form of the step type.
 */
const STEP_TYPES_WITH_ERROR_ROUTING = new Set([
  'Agent',
  'AiAgent',
  'AI Agent',
  'EmbedWorkflow',
  'Split',
  'While',
  'WaitForSignal',
  'Wait For Signal',
]);

/**
 * Determines if a step can have error handlers based on its type.
 *
 * Every Agent step can fail and accepts `onError` routing regardless of
 * whether its capability declares `knownErrors` — the DSL/compiler accept
 * `onError` edges on all Agent steps, so the editor offers the handler
 * unconditionally for the compiler-supported step set.
 *
 * @param stepType - The type of step (e.g., 'Agent', 'AiAgent', 'Split')
 * @returns true if the compiler supports `onError` routing on this step type
 */
export function canStepHaveErrorHandler(stepType: string): boolean {
  return STEP_TYPES_WITH_ERROR_ROUTING.has(stepType);
}
