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

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/**
 * Analysis of the static inputs text for the schema-driven (structured)
 * editor. The structured form edits `envelope.data` fields covered by the
 * workflow's input schema; everything else (the `variables` key, unknown
 * `data.*` keys, any other envelope keys) is "unrepresented" and must be
 * preserved verbatim by `buildStaticInputsText` — surfaced here so the UI
 * can warn instead of silently dropping them.
 */
export type StaticInputsAnalysis =
  | {
      representable: false;
      /**
       * 'invalid-json': text fails staticInputsError (not parseable / not an
       * object). 'data-not-object': envelope parses but `data` is not a
       * plain object, so a field form cannot edit it.
       */
      reason: 'invalid-json' | 'data-not-object';
    }
  | {
      representable: true;
      /** The `data` object the structured form edits ({} when absent). */
      data: Record<string, unknown>;
      /** Envelope keys other than `data` (e.g. `variables`). */
      unrepresentedEnvelopeKeys: string[];
      /** Keys inside `data` that no schema field covers. */
      unrepresentedDataKeys: string[];
    };

/**
 * Analyze the raw "Static inputs (JSON)" text against the selected
 * workflow's input schema field names. Blank text is representable as an
 * empty form.
 */
export function analyzeStaticInputs(
  text: unknown,
  schemaFieldNames: string[]
): StaticInputsAnalysis {
  if (staticInputsError(text) !== null) {
    return { representable: false, reason: 'invalid-json' };
  }

  const parsed = parseStaticInputs(text);
  if (parsed === undefined) {
    // Blank: an empty structured form.
    return {
      representable: true,
      data: {},
      unrepresentedEnvelopeKeys: [],
      unrepresentedDataKeys: [],
    };
  }

  const envelope = parsed as Record<string, unknown>;
  if ('data' in envelope && !isPlainObject(envelope.data)) {
    return { representable: false, reason: 'data-not-object' };
  }

  const data = isPlainObject(envelope.data) ? envelope.data : {};
  const knownFields = new Set(schemaFieldNames);

  return {
    representable: true,
    data,
    unrepresentedEnvelopeKeys: Object.keys(envelope).filter(
      (key) => key !== 'data'
    ),
    unrepresentedDataKeys: Object.keys(data).filter(
      (key) => !knownFields.has(key)
    ),
  };
}

/**
 * Build the next "Static inputs (JSON)" text from the structured form's
 * data object, preserving everything the form cannot represent verbatim:
 * envelope keys other than `data` (e.g. `variables`) and `data.*` keys not
 * covered by the schema survive untouched. Returns '' (blank, which removes
 * `configuration.inputs` on save) when nothing remains in the envelope.
 *
 * `previousText` is expected to be blank or valid JSON (the structured form
 * is unavailable otherwise); an unparsable value is treated as blank.
 */
export function buildStaticInputsText(
  previousText: unknown,
  formData: Record<string, unknown>,
  schemaFieldNames: string[]
): string {
  let envelope: Record<string, unknown> = {};
  if (staticInputsError(previousText) === null) {
    const parsed = parseStaticInputs(previousText);
    if (isPlainObject(parsed)) {
      envelope = parsed;
    }
  }

  const knownFields = new Set(schemaFieldNames);
  const previousData = isPlainObject(envelope.data) ? envelope.data : {};

  // Keep data keys the schema has no field for, then layer the form's data
  // (which only ever writes schema-covered keys) on top.
  const data: Record<string, unknown> = {
    ...Object.fromEntries(
      Object.entries(previousData).filter(([key]) => !knownFields.has(key))
    ),
    ...formData,
  };

  // Drop undefined entries (e.g. a cleared number input) so they don't
  // linger as JSON-unserializable values.
  for (const key of Object.keys(data)) {
    if (data[key] === undefined) {
      delete data[key];
    }
  }

  const next: Record<string, unknown> = { ...envelope, data };

  if (Object.keys(data).length === 0) {
    delete next.data;
    if (Object.keys(next).length === 0) {
      // Blank removes `configuration.inputs` entirely on save.
      return '';
    }
  }

  return JSON.stringify(next, null, 2);
}

/**
 * Validate a custom cron expression against what the server's cron scheduler
 * accepts. `normalize_cron_expression` (workers/cron_scheduler.rs) runs
 * standard 5-field expressions as-is and additionally accepts 6-field
 * expressions whose seconds field is '0' (the seconds field is stripped).
 * Mirror that here so the form does not reject expressions the server runs.
 */
export function isAcceptedCronExpression(expression: unknown): boolean {
  if (typeof expression !== 'string' || expression.trim() === '') {
    return false;
  }
  const parts = expression.trim().split(/\s+/);
  if (parts.length === 5) {
    return true;
  }
  return parts.length === 6 && parts[0] === '0';
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

export interface WebhookConfigurationOptions {
  /** Existing trigger configuration whose unknown keys must be preserved. */
  existing?: Record<string, unknown> | null;
  /**
   * Debug mode toggle. The webhook ingest handler reads
   * `configuration.debug` via as_bool (api/handlers/events.rs), so this must
   * be stored as a real boolean; false removes the key.
   */
  debug?: boolean;
  /**
   * Connection used for webhook signature verification. The server reads
   * `configuration.connection_id` (api/services/webhook_verification.rs) and
   * verifies the request against that connection's signing key. Blank
   * removes the key (verification disabled).
   */
  connectionId?: string | null;
}

/**
 * Build an HTTP/EMAIL trigger configuration, merging the form-managed keys
 * (`debug`, `connection_id`) over the existing configuration object.
 */
export function buildWebhookConfiguration(
  options: WebhookConfigurationOptions
): Record<string, unknown> {
  const { existing, debug, connectionId } = options;
  const configuration: Record<string, unknown> = { ...(existing ?? {}) };

  if (debug) {
    configuration.debug = true;
  } else {
    delete configuration.debug;
  }

  if (connectionId) {
    configuration.connection_id = connectionId;
  } else {
    delete configuration.connection_id;
  }

  return configuration;
}
