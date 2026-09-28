import { describe, expect, it } from 'vitest';
import {
  REQUIRES_RUN_TAG,
  capabilityRequiresRun,
  capabilitySuspends,
  suspendingStepIssues,
} from './capability-flags';

describe('capability flags', () => {
  it('marks only `suspends: true` capabilities as suspending', () => {
    expect(capabilitySuspends({ suspends: true })).toBe(true);
    expect(capabilitySuspends({ suspends: false })).toBe(false);
    expect(capabilitySuspends({})).toBe(false);
    expect(capabilitySuspends(undefined)).toBe(false);
  });

  it('detects the runtime:requires-run tag', () => {
    expect(capabilityRequiresRun({ tags: ['control', REQUIRES_RUN_TAG] })).toBe(
      true
    );
    expect(capabilityRequiresRun({ tags: ['memory:read'] })).toBe(false);
    expect(capabilityRequiresRun({})).toBe(false);
    expect(capabilityRequiresRun(null)).toBe(false);
  });

  it('flags a non-durable step and a missing or zero timeout', () => {
    expect(
      suspendingStepIssues({ durable: undefined, timeout: 60000 })
    ).toEqual({ notDurable: false, missingTimeout: false });
    expect(suspendingStepIssues({ durable: false, timeout: 1 })).toEqual({
      notDurable: true,
      missingTimeout: false,
    });
    for (const timeout of [undefined, null, '', 0, -5, 'abc']) {
      expect(
        suspendingStepIssues({ durable: true, timeout }).missingTimeout
      ).toBe(true);
    }
    expect(suspendingStepIssues({ timeout: '250' }).missingTimeout).toBe(false);
  });
});
