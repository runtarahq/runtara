// Hooks for the WASM-backed report DSL.
//
// `useReportDsl()` suspends the calling tree until the WASM bundle has
// loaded, then returns a synchronous handle. Cell renderers depend on
// the sync handle so they can format values inline during React render.
//
// `useReportRenderContext()` derives a stable per-render `RenderContext`
// (locale + currency + timezone) from browser globals plus an optional
// per-report currency override.

import { useMemo } from 'react';
import type {
  ReportDsl,
  ReportRenderContext,
} from '@/wasm/runtara-report-dsl';
import {
  defaultRenderContext,
  ensureReportDsl,
  reportDsl,
} from '@/wasm/runtara-report-dsl';

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

/**
 * Derive a stable `RenderContext` for the current viewer. The optional
 * `currency` override comes from the report definition (e.g. a metric
 * configured for EUR even if the viewer's locale defaults to USD).
 */
export function useReportRenderContext(
  currency?: string | null
): ReportRenderContext {
  return useMemo(() => {
    const base = defaultRenderContext();
    return currency ? { ...base, currency } : base;
  }, [currency]);
}

/**
 * Eager-load the WASM bundle. Use from app shell or report layouts so
 * `useReportDsl()` resolves without ever suspending render.
 */
export function preloadReportDsl(): Promise<ReportDsl> {
  return ensureReportDsl();
}

/**
 * Non-suspending accessor for code paths that already know the bundle
 * is loaded (e.g. event handlers triggered after a successful render).
 * Throws if the bundle hasn't loaded yet.
 */
export function getReportDsl(): ReportDsl {
  return reportDsl();
}
