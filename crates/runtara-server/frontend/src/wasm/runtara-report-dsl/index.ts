// Async-init wrapper around the `runtara-report-dsl` WASM bundle.
//
// Phase 2 of the reports refactor: server and FE share one minijinja
// template engine, one `ConditionExpression` evaluator, and one
// `FormatSpec` grammar. The bundle is vendored as the wasm-pack
// `--target web` artifact; loading is async on first call and cached
// thereafter.
//
// Locale-aware number/date/currency formatting is delegated back into
// JS via the `__runtaraReportDslFormatValue` callback (set up below).
// The browser's `Intl` does all the locale resolution — no CLDR data
// in the WASM bundle. A future server-side ICU `Formatter` would be a
// drop-in replacement on the Rust side; FE callers don't change.
//
// Usage:
//   import { reportDsl, ensureReportDsl } from '@/wasm/runtara-report-dsl';
//   await ensureReportDsl();                  // preload once
//   const dsl = reportDsl();                  // sync handle thereafter
//   const out = dsl.renderTemplate('Hello {{ name }}', { name: 'world' }, ctx);

import init, {
  evaluateRowCondition,
  formatValue,
  renderTemplate,
  validateTemplate,
  version,
} from './runtara_report_dsl.js';
import wasmUrl from './runtara_report_dsl_bg.wasm?url';

/** Per-render context shared with every format/template call. */
export interface ReportRenderContext {
  /** BCP-47 locale tag, e.g. "en-US". */
  locale: string;
  /** ISO 4217 default currency, e.g. "USD". */
  currency: string;
  /** IANA timezone, e.g. "Europe/Berlin". */
  timezone: string;
}

/** Closed-set format spec serialized across the WASM<->JS boundary. */
type FormatSpec =
  | { kind: 'currency'; code: string | null }
  | { kind: 'currency_compact'; code: string | null }
  | { kind: 'number' }
  | { kind: 'number_compact' }
  | { kind: 'decimal' }
  | { kind: 'percent' }
  | { kind: 'date' }
  | { kind: 'datetime' }
  | { kind: 'pill' }
  | { kind: 'bar_indicator' }
  | { kind: 'string' }
  | { kind: 'raw' };

export interface ReportDsl {
  /** Returns the crate's package version. */
  version: () => string;
  /** Render a `{{ field | filter }}` template against a row. */
  renderTemplate: (
    template: string,
    row: unknown,
    ctx: ReportRenderContext
  ) => string;
  /** One-shot value formatting outside a template. */
  formatValue: (
    value: unknown,
    format: string,
    ctx: ReportRenderContext
  ) => string;
  /** Compile-check a template. Throws on parse error. */
  validateTemplate: (template: string) => void;
  /** Evaluate a `ConditionExpression` against a row. */
  evaluateRowCondition: (expr: unknown, row: unknown) => boolean;
}

let loadPromise: Promise<ReportDsl> | null = null;
let resolved: ReportDsl | null = null;

/**
 * Default `RenderContext` derived from the browser. Callers can override
 * any field — e.g. a report whose definition pins a currency code.
 */
export function defaultRenderContext(): ReportRenderContext {
  return {
    locale:
      typeof navigator !== 'undefined' && navigator.language
        ? navigator.language
        : 'en-US',
    currency: 'USD',
    timezone:
      typeof Intl !== 'undefined'
        ? Intl.DateTimeFormat().resolvedOptions().timeZone
        : 'UTC',
  };
}

/**
 * Preload the WASM bundle. Resolves once the module is instantiated.
 * Call from app shell mount so cell renderers can use the sync
 * `reportDsl()` handle inline during render.
 */
export function ensureReportDsl(): Promise<ReportDsl> {
  if (!loadPromise) {
    registerFormatCallback();
    loadPromise = init({ module_or_path: wasmUrl }).then(() => {
      resolved = {
        version,
        renderTemplate,
        formatValue,
        validateTemplate,
        evaluateRowCondition,
      };
      return resolved;
    });
  }
  return loadPromise;
}

/**
 * Synchronous handle to the loaded DSL. Throws if `ensureReportDsl()`
 * hasn't resolved yet — guard cell renderers behind the preload boundary.
 */
export function reportDsl(): ReportDsl {
  if (!resolved) {
    throw new Error(
      '[reportDsl] WASM bundle not loaded yet. Call `ensureReportDsl()` first.'
    );
  }
  return resolved;
}

// ---------------------------------------------------------------------------
// JS-side formatter callback. WASM calls back into here for every filter
// invocation. We use `Intl` for full CLDR coverage at zero bundle cost.
// ---------------------------------------------------------------------------

declare global {
  interface Window {
    __runtaraReportDslFormatValue?: (
      value: unknown,
      specJson: string,
      locale: string,
      currency: string,
      timezone: string
    ) => string;
  }
}

let callbackRegistered = false;

function registerFormatCallback(): void {
  if (callbackRegistered || typeof window === 'undefined') return;
  callbackRegistered = true;
  window.__runtaraReportDslFormatValue = formatValueViaIntl;
}

function formatValueViaIntl(
  value: unknown,
  specJson: string,
  locale: string,
  currency: string,
  timezone: string
): string {
  if (value === null || value === undefined) return '';

  let spec: FormatSpec;
  try {
    spec = JSON.parse(specJson) as FormatSpec;
  } catch {
    return stringifyRaw(value);
  }

  const effectiveLocale = locale || undefined;
  const effectiveTimezone = timezone || undefined;

  switch (spec.kind) {
    case 'currency': {
      const num = toNumber(value);
      if (num === null) return stringifyRaw(value);
      return new Intl.NumberFormat(effectiveLocale, {
        style: 'currency',
        currency: spec.code?.toUpperCase() || currency || 'USD',
        maximumFractionDigits: 2,
      }).format(num);
    }
    case 'currency_compact': {
      const num = toNumber(value);
      if (num === null) return stringifyRaw(value);
      return new Intl.NumberFormat(effectiveLocale, {
        style: 'currency',
        currency: spec.code?.toUpperCase() || currency || 'USD',
        notation: 'compact',
        maximumFractionDigits: Math.abs(num) < 1_000_000 ? 1 : 0,
      }).format(num);
    }
    case 'number': {
      const num = toNumber(value);
      if (num === null) return stringifyRaw(value);
      return new Intl.NumberFormat(effectiveLocale, {
        maximumFractionDigits: 0,
      }).format(num);
    }
    case 'number_compact': {
      const num = toNumber(value);
      if (num === null) return stringifyRaw(value);
      return new Intl.NumberFormat(effectiveLocale, {
        notation: 'compact',
        maximumFractionDigits: Math.abs(num) < 1_000_000 ? 1 : 0,
      }).format(num);
    }
    case 'decimal': {
      const num = toNumber(value);
      if (num === null) return stringifyRaw(value);
      return new Intl.NumberFormat(effectiveLocale, {
        maximumFractionDigits: 2,
      }).format(num);
    }
    case 'percent': {
      const num = toNumber(value);
      if (num === null) return stringifyRaw(value);
      return new Intl.NumberFormat(effectiveLocale, {
        style: 'percent',
        maximumFractionDigits: 2,
      }).format(num);
    }
    case 'date': {
      const date = toDate(value);
      if (date === null) return stringifyRaw(value);
      return new Intl.DateTimeFormat(effectiveLocale, {
        timeZone: effectiveTimezone,
      }).format(date);
    }
    case 'datetime': {
      const date = toDate(value);
      if (date === null) return stringifyRaw(value);
      return new Intl.DateTimeFormat(effectiveLocale, {
        timeZone: effectiveTimezone,
        dateStyle: 'medium',
        timeStyle: 'short',
      }).format(date);
    }
    case 'string':
      return stringifyRaw(value);
    case 'pill':
    case 'bar_indicator':
    case 'raw':
      return stringifyRaw(value);
    default:
      return stringifyRaw(value);
  }
}

function toNumber(value: unknown): number | null {
  if (typeof value === 'number' && Number.isFinite(value)) return value;
  if (typeof value === 'string') {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : null;
  }
  return null;
}

function toDate(value: unknown): Date | null {
  if (value instanceof Date)
    return Number.isFinite(value.getTime()) ? value : null;
  if (typeof value === 'string') {
    const parsed = new Date(value);
    return Number.isFinite(parsed.getTime()) ? parsed : null;
  }
  return null;
}

function stringifyRaw(value: unknown): string {
  if (value === null || value === undefined) return '';
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'boolean')
    return String(value);
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}
