import '@testing-library/jest-dom';
import { vi } from 'vitest';

let evaluateConditionForTests: (
  expression: unknown,
  data: unknown
) => boolean = () => false;

// Vitest can't load the WASM bundle (jsdom has no fetch resolver for the
// `?url` import), so we replace the report DSL loader with a minimal
// in-memory shim. The shim mirrors the WASM surface for the tests that
// touch FE formatting; end-to-end template/format behavior is covered by
// the Rust tests in `runtara-report-dsl/src/{format,template}.rs`.
vi.mock('@/wasm/runtara-report-dsl/index', () => {
  const defaultContext = () => ({
    locale: 'en-US',
    currency: 'USD',
    timezone: 'UTC',
  });

  const formatValue = (
    value: unknown,
    format: string,
    _ctx: { locale: string; currency: string; timezone: string }
  ): string => {
    if (value === null || value === undefined) return '';
    if (typeof value === 'object') {
      try {
        return JSON.stringify(value);
      } catch {
        return String(value);
      }
    }
    if (format) {
      // Best-effort ASCII fallback: the Rust SimpleAsciiFormatter is the
      // source of truth, but vitest doesn't need byte-exact output — most
      // FE tests assert presence/structure, not formatting precision.
      if (typeof value === 'number') {
        if (format.startsWith('currency')) return `$${value.toFixed(2)}`;
        if (format === 'percent') return `${(value * 100).toFixed(1)}%`;
        if (format === 'number' || format === 'decimal') return String(value);
      }
    }
    return String(value);
  };

  const renderTemplate = (
    template: string,
    row: Record<string, unknown>,
    _ctx: { locale: string; currency: string; timezone: string }
  ): string => {
    return template.replace(
      /\{\{\s*([^}|]+?)(?:\s*\|\s*([^}]+))?\s*\}\}/g,
      (_, field: string, fmt?: string) => {
        const path = field
          .trim()
          .replace(/^row\./, '')
          .split('.');
        let current: unknown = row;
        for (const segment of path) {
          if (current === null || current === undefined) return '';
          if (typeof current !== 'object') return '';
          current = (current as Record<string, unknown>)[segment];
        }
        return formatValue(current, fmt?.trim() ?? '', defaultContext());
      }
    );
  };

  // Minimal canonical-condition evaluator — mirrors
  // `runtara_report_dsl::evaluate_row_condition` for the operators the
  // FE tests touch (AND, OR, NOT, EQ, NE, GT/GTE/LT/LTE, IN, NOT_IN,
  // CONTAINS, IS_DEFINED, IS_EMPTY, IS_NOT_EMPTY). Server-side
  // operators (SIMILARITY, MATCH, etc.) throw the same way the WASM
  // crate does.
  type Cond =
    | { type: 'operation'; op: string; arguments: Cond[] }
    | { type: 'value'; valueType: 'reference'; value: string }
    | { type: 'value'; valueType: 'immediate'; value: unknown };

  const lookup = (row: Record<string, unknown>, path: string): unknown => {
    let current: unknown = row;
    for (const part of path.split('.')) {
      if (current === null || current === undefined) return null;
      if (typeof current !== 'object') return null;
      current = (current as Record<string, unknown>)[part];
    }
    return current ?? null;
  };

  const valueOf = (arg: Cond, row: Record<string, unknown>): unknown => {
    if (arg.type === 'value' && arg.valueType === 'reference')
      return lookup(row, arg.value);
    if (arg.type === 'value' && arg.valueType === 'immediate')
      return arg.value ?? null;
    return null;
  };

  const equal = (a: unknown, b: unknown): boolean =>
    JSON.stringify(a) === JSON.stringify(b);

  const compare = (a: unknown, b: unknown): number => {
    if (typeof a === 'number' && typeof b === 'number') return a - b;
    if (typeof a === 'string' && typeof b === 'string')
      return a.localeCompare(b);
    return 0;
  };

  const evaluate = (cond: Cond, row: Record<string, unknown>): boolean => {
    if (cond.type === 'value') return Boolean(valueOf(cond, row));
    const op = cond.op.toUpperCase();
    const args = cond.arguments;
    switch (op) {
      case 'AND':
        return args.every((a) => evaluate(a, row));
      case 'OR':
        return args.some((a) => evaluate(a, row));
      case 'NOT':
        return !evaluate(args[0], row);
      case 'EQ':
        return equal(valueOf(args[0], row), valueOf(args[1], row));
      case 'NE':
        return !equal(valueOf(args[0], row), valueOf(args[1], row));
      case 'GT':
        return compare(valueOf(args[0], row), valueOf(args[1], row)) > 0;
      case 'GTE':
        return compare(valueOf(args[0], row), valueOf(args[1], row)) >= 0;
      case 'LT':
        return compare(valueOf(args[0], row), valueOf(args[1], row)) < 0;
      case 'LTE':
        return compare(valueOf(args[0], row), valueOf(args[1], row)) <= 0;
      case 'IN': {
        const candidates = valueOf(args[1], row);
        const value = valueOf(args[0], row);
        return Array.isArray(candidates)
          ? candidates.some((c) => equal(value, c))
          : false;
      }
      case 'NOT_IN': {
        const candidates = valueOf(args[1], row);
        const value = valueOf(args[0], row);
        return Array.isArray(candidates)
          ? !candidates.some((c) => equal(value, c))
          : true;
      }
      case 'CONTAINS': {
        const value = valueOf(args[0], row);
        const needle = valueOf(args[1], row);
        return (
          typeof value === 'string' &&
          typeof needle === 'string' &&
          value.includes(needle)
        );
      }
      case 'IS_DEFINED':
        return valueOf(args[0], row) !== null;
      case 'IS_EMPTY': {
        const v = valueOf(args[0], row);
        if (v === null || v === undefined) return true;
        if (typeof v === 'string') return v.length === 0;
        if (Array.isArray(v)) return v.length === 0;
        return false;
      }
      case 'IS_NOT_EMPTY': {
        const v = valueOf(args[0], row);
        if (v === null || v === undefined) return false;
        if (typeof v === 'string') return v.length > 0;
        if (Array.isArray(v)) return v.length > 0;
        return true;
      }
      default:
        throw new Error(`unsupported operator: ${op}`);
    }
  };
  evaluateConditionForTests = (expression, data) =>
    evaluate(expression as Cond, (data ?? {}) as Record<string, unknown>);

  const dsl = {
    version: () => 'test',
    renderTemplate,
    formatValue,
    validateTemplate: () => {},
  };

  return {
    defaultRenderContext: defaultContext,
    ensureReportDsl: () => Promise.resolve(dsl),
    reportDsl: () => dsl,
  };
});

vi.mock('@/shared/lib/rust-validation-wasm', async (importOriginal) => ({
  ...(await importOriginal<
    typeof import('@/shared/lib/rust-validation-wasm')
  >()),
  evaluateCanonicalCondition: (expression: unknown, data: unknown) =>
    evaluateConditionForTests(expression, data),
}));
