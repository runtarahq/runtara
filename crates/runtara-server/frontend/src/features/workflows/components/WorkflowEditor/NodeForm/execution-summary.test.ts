import { describe, it, expect } from 'vitest';
import { summarizeExecution } from './NodeFormItem';

const base = { showDurable: true, showRetries: true };

describe('summarizeExecution', () => {
  it('says so when nothing deviates', () => {
    expect(summarizeExecution({ ...base })).toBe('all at defaults');
  });

  it('treats an unset durable as durable', () => {
    expect(summarizeExecution({ ...base, durable: undefined })).toBe(
      'all at defaults'
    );
    expect(summarizeExecution({ ...base, durable: true })).toBe(
      'all at defaults'
    );
    expect(summarizeExecution({ ...base, durable: false })).toBe('not durable');
  });

  it('reports a breakpoint, which is the whole reason to collapse safely', () => {
    expect(summarizeExecution({ ...base, breakpoint: true })).toBe(
      'breakpoint on'
    );
    expect(summarizeExecution({ ...base, breakpoint: false })).toBe(
      'all at defaults'
    );
  });

  it('reports zero rather than swallowing it as falsy', () => {
    expect(summarizeExecution({ ...base, maxRetries: 0 })).toBe('0 retries');
  });

  it('counts one retry as a retry', () => {
    expect(summarizeExecution({ ...base, maxRetries: 1 })).toBe('1 retry');
  });

  it('joins every deviation', () => {
    expect(
      summarizeExecution({
        ...base,
        breakpoint: true,
        durable: false,
        maxRetries: 3,
        retryDelay: 1000,
        timeout: 5000,
      })
    ).toBe(
      'breakpoint on · not durable · 3 retries · 1000ms delay · 5000ms timeout'
    );
  });

  // A Switch shows only Breakpoint; claiming "3 retries" for a group that does
  // not render retries would point at a control the user cannot find.
  it('ignores fields the group does not show', () => {
    expect(
      summarizeExecution({
        showDurable: false,
        showRetries: false,
        durable: false,
        maxRetries: 3,
        timeout: 5000,
      })
    ).toBe('all at defaults');
  });
});
