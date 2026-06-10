// Helpers for building trigger `configuration` payloads.
//
// The server reads more keys out of `configuration` than the form edits
// (e.g. `inputs` + `debug` for CRON fires, `debug` + `connection_id` for
// HTTP/EMAIL webhook signature verification), so edit-save must merge the
// form-managed keys over the existing object instead of rebuilding it from
// scratch and silently destroying API-authored keys.

/**
 * Validation error for the "Static inputs (JSON)" textarea value.
 * Returns null when the value is blank or a valid JSON object.
 */
export function staticInputsError(text: unknown): string | null {
  if (typeof text !== 'string' || text.trim() === '') {
    return null;
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return 'Static inputs must be valid JSON.';
  }

  if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) {
    return 'Static inputs must be a JSON object, e.g. {"data": {...}, "variables": {...}}.';
  }

  return null;
}

/**
 * Parse the optional "Static inputs (JSON)" textarea value.
 * Blank/non-string values yield undefined. Throws on invalid JSON, so
 * callers must validate via the form schema (staticInputsError) first.
 */
export function parseStaticInputs(text: unknown): unknown {
  if (typeof text !== 'string') {
    return undefined;
  }
  const trimmed = text.trim();
  if (!trimmed) {
    return undefined;
  }
  return JSON.parse(trimmed);
}

export interface CronConfigurationOptions {
  /** Existing trigger configuration whose unknown keys must be preserved. */
  existing?: Record<string, unknown> | null;
  /** New cron expression (overwrites any existing one). */
  expression?: string;
  /** Raw "Static inputs (JSON)" textarea value; blank removes `inputs`. */
  inputsText?: string | null;
  /**
   * Debug mode toggle. The cron scheduler reads `configuration.debug` via
   * as_bool, so this must be stored as a real boolean; false removes the key.
   */
  debug?: boolean;
}

/**
 * Build a CRON trigger configuration, merging the form-managed keys
 * (`expression`, `inputs`, `debug`) over the existing configuration object.
 */
export function buildCronConfiguration(
  options: CronConfigurationOptions
): Record<string, unknown> {
  const { existing, expression, inputsText, debug } = options;
  const configuration: Record<string, unknown> = { ...(existing ?? {}) };

  if (expression) {
    configuration.expression = expression;
  }

  const inputs = parseStaticInputs(inputsText);
  if (inputs !== undefined) {
    configuration.inputs = inputs;
  } else {
    delete configuration.inputs;
  }

  if (debug) {
    configuration.debug = true;
  } else {
    delete configuration.debug;
  }

  return configuration;
}
