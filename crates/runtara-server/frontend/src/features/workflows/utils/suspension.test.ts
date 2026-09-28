import { describe, expect, it } from 'vitest';
import {
  canResume,
  isAtBreakpoint,
  isWaitingSuspension,
  suspendedStatusLabel,
  suspensionReasonLabel,
  waitingStatusText,
} from './suspension';

describe('suspension helpers', () => {
  it('offers Resume only for a paused suspended run', () => {
    expect(canResume({ status: 'suspended', suspensionReason: 'paused' })).toBe(
      true
    );
    for (const suspensionReason of [
      'waiting_signal',
      'waiting_instances',
      'sleeping',
      'shutdown',
      null,
    ] as const) {
      expect(canResume({ status: 'suspended', suspensionReason })).toBe(false);
    }
    // Finished runs answer NotResumable — never offer Resume.
    for (const status of ['failed', 'cancelled', 'completed', 'running']) {
      expect(canResume({ status, suspensionReason: 'paused' })).toBe(false);
    }
    expect(canResume(undefined)).toBe(false);
  });

  it('counts only a paused run as stopped at a breakpoint; others are waiting', () => {
    const paused = { status: 'suspended', suspensionReason: 'paused' } as const;
    expect(isAtBreakpoint(paused)).toBe(true);
    expect(isWaitingSuspension(paused)).toBe(false);

    for (const suspensionReason of [
      'waiting_signal',
      'waiting_instances',
      'sleeping',
    ] as const) {
      const run = { status: 'suspended', suspensionReason };
      expect(isAtBreakpoint(run)).toBe(false);
      expect(isWaitingSuspension(run)).toBe(true);
    }
    expect(isWaitingSuspension({ status: 'running' })).toBe(false);
  });

  it('labels suspension reasons', () => {
    expect(suspensionReasonLabel('paused')).toBe('Paused');
    expect(suspensionReasonLabel('waiting_signal')).toBe('Waiting for signal');
    expect(suspensionReasonLabel('waiting_instances')).toBe(
      'Waiting for instances'
    );
    expect(suspensionReasonLabel('sleeping')).toBe('Sleeping');
    expect(suspensionReasonLabel('shutdown')).toBe('Shutdown');
    expect(suspensionReasonLabel(null)).toBeNull();

    expect(
      suspendedStatusLabel({
        status: 'suspended',
        suspensionReason: 'sleeping',
      })
    ).toBe('Sleeping');
    expect(
      suspendedStatusLabel({ status: 'failed', suspensionReason: 'paused' })
    ).toBeNull();
  });

  it('builds the waiting toolbar text', () => {
    expect(waitingStatusText('waiting_signal')).toBe('Waiting for signal');
    expect(waitingStatusText('waiting_instances')).toBe(
      'Waiting for instances'
    );
    expect(waitingStatusText('sleeping')).toBe('Waiting (sleeping)');
    expect(waitingStatusText(null)).toBe('Waiting');
  });
});
