/**
 * Synchronous cache over the canonical per-step-type output shapes
 * (runtara-dsl `step_output_shape`), delivered through the validation WASM.
 *
 * The reference-suggestion builder (`NodeForm/shared.ts`) runs synchronously
 * inside render paths, so it can't await the WASM module. Instead the editor
 * warms this cache once (NodeFormProvider mount) and the builder reads from
 * it; before the warm-up finishes, control steps degrade to the generic
 * "outputs" suggestion they had historically.
 *
 * Keeping the shapes out of frontend source is deliberate: the table in
 * `step_output_shape.rs` is the single source of truth pinned to the runtime
 * emitters, and hand-copied shapes here have already drifted once (While's
 * `iterations` was suggested at `steps.<id>.iterations`, which resolves to
 * null at runtime — the canonical path is `steps.<id>.outputs.iterations`).
 */
import {
  getStaticStepTypeSchemaWithRust,
  getStaticStepTypesWithRust,
} from './rust-workflow-validation';

export interface ShapeFieldJson {
  name: string;
  /** "string" | "number" | "integer" | "boolean" | "array" | "object" | "dynamic" */
  type: string;
  description?: string;
  /**
   * Step config key that must be truthy for the runtime to write this field
   * (e.g. Split's failure siblings require config.dontStopOnFailed).
   */
  gatedBy?: string;
}

export interface OutputShapeJson {
  summary?: string;
  reference?: string;
  outputs?: {
    kind: 'array' | 'object' | 'dynamic';
    fields?: ShapeFieldJson[];
    note?: string;
  };
  siblingFields?: ShapeFieldJson[];
}

const cache = new Map<string, OutputShapeJson | null>();
let warmed = false;
let warming: Promise<void> | null = null;

/**
 * Load every step type's outputShape from the validation WASM into the
 * synchronous cache. Idempotent; concurrent callers share one in-flight
 * warm-up. On failure the cache stays cold and a later call retries.
 */
export function warmStepOutputShapes(): Promise<void> {
  if (warmed) {
    return Promise.resolve();
  }
  if (!warming) {
    warming = (async () => {
      try {
        const { step_types } = await getStaticStepTypesWithRust();
        await Promise.all(
          (step_types ?? []).map(async (stepType) => {
            const schema = (await getStaticStepTypeSchemaWithRust(
              stepType.id
            )) as { outputShape?: OutputShapeJson | null } | null;
            cache.set(stepType.id, schema?.outputShape ?? null);
          })
        );
        warmed = true;
      } catch (error) {
        console.warn('Step output shapes unavailable', error);
        warming = null;
      }
    })();
  }
  return warming;
}

/**
 * Canonical output shape for a PascalCase step type id ("Split", "While", …),
 * or null when unknown or the cache has not been warmed yet.
 */
export function getStepOutputShape(stepType: string): OutputShapeJson | null {
  return cache.get(stepType) ?? null;
}

/** Test hook: seed the cache without loading the WASM module. */
export function __setStepOutputShapesForTests(
  shapes: Record<string, OutputShapeJson | null>
): void {
  cache.clear();
  for (const [stepType, shape] of Object.entries(shapes)) {
    cache.set(stepType, shape);
  }
  warmed = true;
  warming = null;
}

/** Test hook: return to the cold-cache state. */
export function __resetStepOutputShapesForTests(): void {
  cache.clear();
  warmed = false;
  warming = null;
}
