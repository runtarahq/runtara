import { InputSubmissionError } from './input-submission';

export type InputIntentRequest = {
  instanceId: string;
  requestId: string;
  payload: Record<string, unknown>;
} & (
  | { kind: 'execution'; workflowId: string }
  | {
      kind: 'report';
      reportId: string;
      blockId: string;
      filters: Record<string, unknown>;
      blockFilters: Record<string, unknown>;
    }
);

export type RetainedInput = {
  operationId: string;
  label: string;
  request: InputIntentRequest;
  state: 'submitting' | 'uncertain' | 'rejected' | 'accepted';
  error?: string;
};

export function canonicalIntent(value: unknown): string {
  return JSON.stringify(value, (_key, item: unknown) =>
    item && typeof item === 'object' && !Array.isArray(item)
      ? Object.fromEntries(
          Object.entries(item).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
        )
      : item
  );
}

export function assertInputReceipt(value: unknown, requestId: string): void {
  const receipt = value as {
    receiptId?: unknown;
    requestId?: unknown;
    acceptedAt?: unknown;
  } | null;
  if (
    !receipt ||
    typeof receipt.receiptId !== 'string' ||
    !receipt.receiptId ||
    receipt.requestId !== requestId ||
    typeof receipt.acceptedAt !== 'string' ||
    !receipt.acceptedAt
  ) {
    throw new InputSubmissionError(
      'Response could not be confirmed. Retry the same response.'
    );
  }
}

/** Owned by a mounted page, never persisted to browser storage. Discovery is
 * deliberately absent: removing a request cannot resolve an uncertain write. */
export class RetainedInputs {
  private inputs: RetainedInput[] = [];
  private listeners = new Set<() => void>();
  snapshot = () => this.inputs;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };
  private update(input: RetainedInput) {
    this.inputs = this.inputs.map((item) =>
      item.operationId === input.operationId ? input : item
    );
    this.listeners.forEach((listener) => listener());
  }
  prepare(request: InputIntentRequest, label: string): RetainedInput {
    const canonical = canonicalIntent(request);
    const existing = this.inputs.find(
      (item) =>
        item.state !== 'rejected' && canonicalIntent(item.request) === canonical
    );
    if (existing) return existing;
    const input: RetainedInput = {
      operationId: crypto.randomUUID(),
      label,
      request: JSON.parse(canonical) as InputIntentRequest,
      state: 'uncertain',
    };
    this.inputs = [...this.inputs, input];
    this.listeners.forEach((listener) => listener());
    return input;
  }
  async send(
    operationId: string,
    transport: (input: RetainedInput) => Promise<unknown>
  ): Promise<boolean> {
    const input = this.inputs.find((item) => item.operationId === operationId);
    if (!input || input.state === 'submitting' || input.state === 'accepted')
      return false;
    this.update({ ...input, state: 'submitting', error: undefined });
    try {
      const receipt = await transport(input);
      assertInputReceipt(receipt, input.request.requestId);
      this.update({ ...input, state: 'accepted' });
      return true;
    } catch (error) {
      const rejected =
        error instanceof InputSubmissionError &&
        error.status !== undefined &&
        error.status >= 400 &&
        error.status < 500 &&
        ![408, 429].includes(error.status);
      this.update({
        ...input,
        state: rejected ? 'rejected' : 'uncertain',
        error:
          error instanceof Error
            ? error.message
            : 'Response could not be confirmed.',
      });
      return false;
    }
  }
}
