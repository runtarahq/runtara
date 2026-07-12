// Hooks for the WASM-backed report DSL.
//
// `useReportDsl()` suspends the calling tree until the WASM bundle has
// loaded, then returns a synchronous handle. Cell renderers depend on
// the sync handle so they can format values inline during React render.

import type { ReportDsl } from '@/wasm/runtara-report-dsl/index';
import { ensureReportDsl } from '@/wasm/runtara-report-dsl/index';

let cachedHandle: ReportDsl | null = null;
const loadPromise: Promise<void> = ensureReportDsl().then((dsl) => {
  cachedHandle = dsl;
});

/**
 * Synchronous handle to the loaded report DSL. Suspends via the load
 * promise until the WASM bundle resolves; cached for the rest of the
 * session afterward.
 */
export function useReportDsl(): ReportDsl {
  if (cachedHandle) return cachedHandle;
  throw loadPromise;
}
