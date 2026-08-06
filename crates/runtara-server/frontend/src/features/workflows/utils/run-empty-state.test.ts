import { describe, it, expect } from 'vitest';
import {
  getRunEventsEmptyState,
  getRunOutputEmptyState,
} from './run-empty-state';

const IN_FLIGHT = [
  'running',
  'queued',
  'pending',
  'compiling',
  'suspended',
  'not_started',
];

describe('getRunEventsEmptyState', () => {
  it('keeps the "may appear soon" wording while the run is in flight', () => {
    IN_FLIGHT.forEach((status) => {
      const state = getRunEventsEmptyState(status, 'Timeline Events');
      expect(state.title).toBe('No Timeline Events Yet');
      expect(state.description).toContain('still running');
    });
  });

  it('does not imply a completed run is still running', () => {
    const state = getRunEventsEmptyState('completed', 'Timeline Events');
    expect(state.title).toBe('No Timeline Events');
    expect(state.description).toBe(
      'This run completed without recording any timeline events.'
    );
    expect(state.description).not.toContain('still running');
  });

  it('names the outcome of a run that did not complete normally', () => {
    expect(getRunEventsEmptyState('failed', 'Events').description).toBe(
      'This run failed without recording any events.'
    );
    expect(getRunEventsEmptyState('cancelled', 'Events').description).toBe(
      'This run was cancelled without recording any events.'
    );
    expect(getRunEventsEmptyState('timeout', 'Events').description).toBe(
      'This run timed out without recording any events.'
    );
  });

  it('treats status aliases the same as their canonical status', () => {
    expect(getRunEventsEmptyState('success', 'Events').description).toBe(
      getRunEventsEmptyState('completed', 'Events').description
    );
    expect(getRunEventsEmptyState('error', 'Events').description).toBe(
      getRunEventsEmptyState('failed', 'Events').description
    );
    expect(getRunEventsEmptyState('aborted', 'Events').description).toBe(
      getRunEventsEmptyState('cancelled', 'Events').description
    );
  });

  it('is case-insensitive', () => {
    const state = getRunEventsEmptyState('COMPLETED', 'Events');
    expect(state.title).toBe('No Events');
    expect(state.description).toBe(
      'This run completed without recording any events.'
    );
  });

  it('falls back to the in-flight wording when the status is unknown', () => {
    [undefined, null, '', 'unknown', 'something_new'].forEach((status) => {
      const state = getRunEventsEmptyState(status, 'Events');
      expect(state.title).toBe('No Events Yet');
      expect(state.description).toContain('still running');
    });
  });

  it('keeps the List view’s refresh hint, and only there', () => {
    expect(getRunEventsEmptyState('running', 'Events').description).toContain(
      'Check back or refresh the page'
    );
    expect(
      getRunEventsEmptyState('running', 'Timeline Events').description
    ).not.toContain('Check back or refresh the page');
    expect(
      getRunEventsEmptyState('completed', 'Events').description
    ).not.toContain('Check back or refresh the page');
  });
});

describe('getRunOutputEmptyState', () => {
  it('promises output only while the run can still produce it', () => {
    IN_FLIGHT.forEach((status) => {
      const state = getRunOutputEmptyState(status);
      expect(state.title).toBe('No output data yet');
      expect(state.description).toBe(
        'Output will be available once the workflow completes'
      );
    });
  });

  it('does not promise output from a finished run', () => {
    const state = getRunOutputEmptyState('completed');
    expect(state.title).toBe('No output data');
    expect(state.description).toBe(
      'This run completed without returning any output'
    );
    expect(state.description).not.toContain('once the workflow completes');
  });

  it('names the outcome of a run that did not complete normally', () => {
    expect(getRunOutputEmptyState('failed').description).toBe(
      'This run failed without returning any output'
    );
    expect(getRunOutputEmptyState('cancelled').description).toBe(
      'This run was cancelled without returning any output'
    );
    expect(getRunOutputEmptyState('timeout').description).toBe(
      'This run timed out without returning any output'
    );
  });

  it('falls back to the in-flight wording when the status is unknown', () => {
    [undefined, null, 'unknown'].forEach((status) => {
      expect(getRunOutputEmptyState(status).title).toBe('No output data yet');
    });
  });
});
