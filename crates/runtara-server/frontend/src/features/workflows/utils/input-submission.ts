export interface InputSubmission {
  requestId: string;
  operationId: string;
  payload: Record<string, unknown>;
}

/** Keep the same operation after an uncertain acknowledgement. Editing the
 * response intentionally starts another operation; object key order does not. */
export class InputSubmissionTracker {
  private operations = new Map<string, { canonical: string; id: string }>();

  prepare(
    instanceId: string,
    requestId: string,
    payload: Record<string, unknown>
  ): InputSubmission {
    const canonical = JSON.stringify(payload, (_key, value: unknown) => {
      if (value && typeof value === 'object' && !Array.isArray(value)) {
        return Object.fromEntries(
          Object.entries(value).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
        );
      }
      return value;
    });
    const key = JSON.stringify([instanceId, requestId]);
    let operation = this.operations.get(key);
    if (!operation || operation.canonical !== canonical) {
      operation = { canonical, id: crypto.randomUUID() };
      this.operations.set(key, operation);
    }
    return {
      requestId,
      operationId: operation.id,
      payload: JSON.parse(canonical) as Record<string, unknown>,
    };
  }
}

export class InputSubmissionError extends Error {
  constructor(
    message: string,
    public readonly code?: string,
    public readonly status?: number
  ) {
    super(message);
    this.name = 'InputSubmissionError';
  }
}

export function isStaleInputSubmission(error: unknown): boolean {
  return (
    error instanceof InputSubmissionError &&
    ['INPUT_INACTIVE', 'INPUT_ALREADY_ANSWERED', 'INPUT_NOT_FOUND'].includes(
      error.code ?? ''
    )
  );
}
