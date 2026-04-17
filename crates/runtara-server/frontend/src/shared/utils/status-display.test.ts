import { describe, it, expect } from 'vitest';
import {
  getStatusDisplay,
  getTerminationTypeDisplay,
  isActiveStatus,
  isTerminalStatus,
} from './status-display';

describe('status-display utilities', () => {
  describe('getStatusDisplay', () => {
    it('returns completed status for "completed"', () => {
      const result = getStatusDisplay('completed');
      expect(result.text).toBe('Completed');
      expect(result.variant).toBe('default');
      expect(result.showSpinner).toBe(false);
    });

    it('returns completed status for "success"', () => {
      const result = getStatusDisplay('success');
      expect(result.text).toBe('Completed');
      expect(result.variant).toBe('default');
      expect(result.showSpinner).toBe(false);
    });

    it('returns failed status for "failed"', () => {
      const result = getStatusDisplay('failed');
      expect(result.text).toBe('Failed');
      expect(result.variant).toBe('destructive');
      expect(result.showSpinner).toBe(false);
    });

    it('returns failed status for "error"', () => {
      const result = getStatusDisplay('error');
      expect(result.text).toBe('Failed');
      expect(result.variant).toBe('destructive');
      expect(result.showSpinner).toBe(false);
    });

    it('returns cancelled status for "cancelled"', () => {
      const result = getStatusDisplay('cancelled');
      expect(result.text).toBe('Cancelled');
      expect(result.variant).toBe('outline');
      expect(result.showSpinner).toBe(false);
    });

    it('returns cancelled status for "aborted"', () => {
      const result = getStatusDisplay('aborted');
      expect(result.text).toBe('Cancelled');
      expect(result.variant).toBe('outline');
      expect(result.showSpinner).toBe(false);
    });

    it('returns timeout status for "timeout"', () => {
      const result = getStatusDisplay('timeout');
      expect(result.text).toBe('Timeout');
      expect(result.variant).toBe('destructive');
      expect(result.showSpinner).toBe(false);
    });

    it('returns running status with spinner for "running"', () => {
      const result = getStatusDisplay('running');
      expect(result.text).toBe('Running');
      expect(result.variant).toBe('secondary');
      expect(result.showSpinner).toBe(true);
    });

    it('returns compiling status with spinner for "compiling"', () => {
      const result = getStatusDisplay('compiling');
      expect(result.text).toBe('Compiling');
      expect(result.variant).toBe('secondary');
      expect(result.showSpinner).toBe(true);
    });

    it('returns queued status with spinner for "queued"', () => {
      const result = getStatusDisplay('queued');
      expect(result.text).toBe('Queued');
      expect(result.variant).toBe('outline');
      expect(result.showSpinner).toBe(true);
    });

    it('returns queued status with spinner for "pending"', () => {
      const result = getStatusDisplay('pending');
      expect(result.text).toBe('Queued');
      expect(result.variant).toBe('outline');
      expect(result.showSpinner).toBe(true);
    });

    it('returns not started status for "not_started"', () => {
      const result = getStatusDisplay('not_started');
      expect(result.text).toBe('Not started');
      expect(result.variant).toBe('outline');
      expect(result.showSpinner).toBe(false);
    });

    it('returns unknown status for unrecognized values', () => {
      const result = getStatusDisplay('unknown_status');
      expect(result.text).toBe('Unknown');
      expect(result.variant).toBe('outline');
      expect(result.showSpinner).toBe(false);
    });

    it('handles null input', () => {
      const result = getStatusDisplay(null);
      expect(result.text).toBe('Unknown');
    });

    it('handles undefined input', () => {
      const result = getStatusDisplay(undefined);
      expect(result.text).toBe('Unknown');
    });

    it('handles case-insensitive input', () => {
      expect(getStatusDisplay('COMPLETED').text).toBe('Completed');
      expect(getStatusDisplay('Running').text).toBe('Running');
      expect(getStatusDisplay('QUEUED').text).toBe('Queued');
    });
  });

  describe('getTerminationTypeDisplay', () => {
    it('returns null for null input', () => {
      expect(getTerminationTypeDisplay(null)).toBeNull();
    });

    it('returns null for undefined input', () => {
      expect(getTerminationTypeDisplay(undefined)).toBeNull();
    });

    it('returns null for normal_completion', () => {
      expect(getTerminationTypeDisplay('normal_completion')).toBeNull();
    });

    it('returns user cancelled for user_initiated', () => {
      const result = getTerminationTypeDisplay('user_initiated');
      expect(result?.text).toBe('User Cancelled');
      expect(result?.variant).toBe('outline');
    });

    it('returns queue timeout for queue_timeout', () => {
      const result = getTerminationTypeDisplay('queue_timeout');
      expect(result?.text).toBe('Queue Timeout (24h)');
      expect(result?.variant).toBe('destructive');
    });

    it('returns execution timeout for execution_timeout', () => {
      const result = getTerminationTypeDisplay('execution_timeout');
      expect(result?.text).toBe('Execution Timeout');
      expect(result?.variant).toBe('destructive');
    });

    it('returns system error for system_error', () => {
      const result = getTerminationTypeDisplay('system_error');
      expect(result?.text).toBe('System Error');
      expect(result?.variant).toBe('destructive');
    });

    it('returns raw value for unknown termination types', () => {
      const result = getTerminationTypeDisplay('custom_termination');
      expect(result?.text).toBe('custom_termination');
      expect(result?.variant).toBe('outline');
    });
  });

  describe('isActiveStatus', () => {
    it('returns true for queued status', () => {
      expect(isActiveStatus('queued')).toBe(true);
    });

    it('returns true for compiling status', () => {
      expect(isActiveStatus('compiling')).toBe(true);
    });

    it('returns true for running status', () => {
      expect(isActiveStatus('running')).toBe(true);
    });

    it('returns true for pending status', () => {
      expect(isActiveStatus('pending')).toBe(true);
    });

    it('returns true for unknown status', () => {
      expect(isActiveStatus('unknown')).toBe(true);
    });

    it('returns false for completed status', () => {
      expect(isActiveStatus('completed')).toBe(false);
    });

    it('returns false for failed status', () => {
      expect(isActiveStatus('failed')).toBe(false);
    });

    it('returns false for cancelled status', () => {
      expect(isActiveStatus('cancelled')).toBe(false);
    });

    it('handles null input', () => {
      expect(isActiveStatus(null)).toBe(false);
    });

    it('handles undefined input', () => {
      expect(isActiveStatus(undefined)).toBe(false);
    });

    it('handles case-insensitive input', () => {
      expect(isActiveStatus('RUNNING')).toBe(true);
      expect(isActiveStatus('Queued')).toBe(true);
    });
  });

  describe('isTerminalStatus', () => {
    it('returns true for completed status', () => {
      expect(isTerminalStatus('completed')).toBe(true);
    });

    it('returns true for failed status', () => {
      expect(isTerminalStatus('failed')).toBe(true);
    });

    it('returns true for cancelled status', () => {
      expect(isTerminalStatus('cancelled')).toBe(true);
    });

    it('returns false for running status', () => {
      expect(isTerminalStatus('running')).toBe(false);
    });

    it('returns false for queued status', () => {
      expect(isTerminalStatus('queued')).toBe(false);
    });

    it('is inverse of isActiveStatus', () => {
      const statuses = [
        'completed',
        'failed',
        'running',
        'queued',
        'cancelled',
        null,
      ];
      statuses.forEach((status) => {
        expect(isTerminalStatus(status)).toBe(!isActiveStatus(status));
      });
    });
  });
});
