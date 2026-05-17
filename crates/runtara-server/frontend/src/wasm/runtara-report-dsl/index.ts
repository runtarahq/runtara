// Async-init wrapper around the `runtara-report-dsl` WASM bundle.
//
// Phase 2 of the reports refactor: server and FE share one minijinja
// template engine and one `ConditionExpression` evaluator. The bundle is
// vendored as the wasm-pack `--target web` artifact; loading is async on
// first call and cached thereafter.
//
// Usage:
//   import { reportDsl } from '@/wasm/runtara-report-dsl';
//   const dsl = await reportDsl();
//   const out = dsl.renderTemplate('Hello {{ name }}', { name: 'world' });

import init, {
  evaluateRowCondition,
  renderTemplate,
  validateTemplate,
  version,
} from './runtara_report_dsl.js';
import wasmUrl from './runtara_report_dsl_bg.wasm?url';

export interface ReportDsl {
  /** Returns the crate's package version. */
  version: () => string;
  /** Render a `{{ field | filter }}` template against a row. Throws on parse/render error. */
  renderTemplate: (template: string, row: unknown) => string;
  /** Compile-check a template. Throws on parse error. */
  validateTemplate: (template: string) => void;
  /** Evaluate a `ConditionExpression` against a row. Throws on server-only operators. */
  evaluateRowCondition: (expr: unknown, row: unknown) => boolean;
}

let loaded: Promise<ReportDsl> | null = null;

/**
 * Resolve the WASM bundle. Lazy — call before first use; the promise is
 * memoized so subsequent calls reuse the same module instance.
 */
export function reportDsl(): Promise<ReportDsl> {
  if (!loaded) {
    loaded = init({ module_or_path: wasmUrl }).then(() => ({
      version,
      renderTemplate,
      validateTemplate,
      evaluateRowCondition,
    }));
  }
  return loaded;
}
