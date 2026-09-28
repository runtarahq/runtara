import type { CapabilityInfo } from '@/generated/RuntaraRuntimeApi';

/**
 * Tag on a capability that only works as a step of a workflow run (control
 * `start`, `send-signal`, `cancel`, `pause`, `resume`). A playground Test
 * call answers `CONTROL_REQUIRES_INSTANCE`.
 */
export const REQUIRES_RUN_TAG = 'runtime:requires-run';

type CapabilityFlags =
  Pick<CapabilityInfo, 'suspends' | 'tags'> | null | undefined;

/** The capability may park the run (a long-polling agent). */
export function capabilitySuspends(capability: CapabilityFlags): boolean {
  return capability?.suspends === true;
}

/** The capability only runs as a step of a workflow run, never in Test. */
export function capabilityRequiresRun(capability: CapabilityFlags): boolean {
  return capability?.tags?.includes(REQUIRES_RUN_TAG) ?? false;
}

export const REQUIRES_RUN_TEST_HINT =
  'This capability only runs as a step of a workflow run. Add it to a workflow and execute the workflow to try it.';

/**
 * What a step whose capability suspends is missing: it must stay durable
 * (E028) and set a timeout above zero ms (E029) — the step's hard deadline,
 * parked time included. An unset `durable` means durable.
 */
export function suspendingStepIssues(values: {
  durable?: unknown;
  timeout?: unknown;
}): { notDurable: boolean; missingTimeout: boolean } {
  const timeout =
    values.timeout === '' || values.timeout == null
      ? NaN
      : Number(values.timeout);
  return {
    notDurable: values.durable === false,
    missingTimeout: !(Number.isFinite(timeout) && timeout > 0),
  };
}
