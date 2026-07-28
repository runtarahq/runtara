import { describe, expect, it } from 'vitest';

import {
  mergeLogSteps,
  type StepSummaryLike,
  type WorkflowLogEvent,
} from './merge-log-steps';

function summary(stepId: string, at: string): StepSummaryLike {
  return {
    stepId,
    stepName: stepId,
    stepType: 'Delay',
    status: 'completed',
    startedAt: at,
    completedAt: at,
  };
}

function logEvent(
  stepId: string,
  at: string,
  extra: Record<string, unknown> = {}
): WorkflowLogEvent {
  return {
    createdAt: at,
    payload: {
      step_id: stepId,
      step_name: `${stepId} name`,
      level: 'info',
      message: `${stepId} fired`,
      ...extra,
    },
  };
}

const WHOLE_RUN = { isCompleteSet: true, sortOrder: 'asc' as const };

describe('mergeLogSteps', () => {
  it('places Log steps among the summaries in time order', () => {
    // The bug: a six-step run reported three, because Log steps emit no
    // step-debug pair and so never reach the summary endpoint.
    const merged = mergeLogSteps(
      [
        summary('delay', '2026-07-27T10:00:01Z'),
        summary('finish', '2026-07-27T10:00:03Z'),
      ],
      [
        logEvent('log-a', '2026-07-27T10:00:00Z'),
        logEvent('log-b', '2026-07-27T10:00:02Z'),
      ],
      WHOLE_RUN
    );

    expect(merged.map((step) => step.stepId)).toEqual([
      'log-a',
      'delay',
      'log-b',
      'finish',
    ]);
  });

  it('describes a Log step the way the views expect', () => {
    const [step] = mergeLogSteps(
      [],
      [logEvent('log-a', '2026-07-27T10:00:00Z', { context: { order: 7 } })],
      WHOLE_RUN
    );

    expect(step).toMatchObject({
      stepId: 'log-a',
      stepName: 'log-a name',
      stepType: 'Log',
      status: 'completed',
      durationMs: 0,
    });
    expect(step.outputs).toEqual({
      level: 'info',
      message: 'log-a fired',
      context: { order: 7 },
    });
  });

  it('shows every log when the run produced no summaries at all', () => {
    // A workflow built only from Log steps: the Events view was completely
    // empty even though the log events existed.
    const merged = mergeLogSteps(
      [],
      [
        logEvent('log-a', '2026-07-27T10:00:00Z'),
        logEvent('log-b', '2026-07-27T10:00:01Z'),
      ],
      WHOLE_RUN
    );

    expect(merged).toHaveLength(2);
  });

  it('carries the scope so a loop-body log is not hoisted to the root', () => {
    const [step] = mergeLogSteps(
      [],
      [
        logEvent('log-a', '2026-07-27T10:00:00Z', {
          scope_id: 'scope-1',
          parent_scope_id: 'root',
        }),
      ],
      WHOLE_RUN
    );

    expect(step.scopeId).toBe('scope-1');
    expect(step.parentScopeId).toBe('root');
  });

  it('treats a log from before scope was recorded as root-level', () => {
    const [step] = mergeLogSteps(
      [],
      [logEvent('log-a', '2026-07-27T10:00:00Z')],
      WHOLE_RUN
    );

    expect(step.scopeId).toBeNull();
    expect(step.parentScopeId).toBeNull();
  });

  describe('when the summaries are only one page of several', () => {
    it('returns the page untouched rather than dropping or repeating logs', () => {
      // Pages are cut on summary offsets, so a log between one page's last
      // summary and the next page's first belongs to neither. Merging per page
      // would either lose it or show it twice.
      const partial = [summary('a', '2026-07-27T10:00:04Z')];
      const merged = mergeLogSteps(
        partial,
        [logEvent('stranded', '2026-07-27T10:00:20Z')],
        { isCompleteSet: false, sortOrder: 'asc' }
      );

      expect(merged).toBe(partial);
    });
  });

  it('honours descending order', () => {
    const merged = mergeLogSteps(
      [summary('delay', '2026-07-27T10:00:01Z')],
      [logEvent('log-a', '2026-07-27T10:00:02Z')],
      { isCompleteSet: true, sortOrder: 'desc' }
    );

    expect(merged.map((step) => step.stepId)).toEqual(['log-a', 'delay']);
  });

  it('leaves the page untouched when there is nothing to merge', () => {
    const summaries = [summary('delay', '2026-07-27T10:00:01Z')];
    expect(mergeLogSteps(summaries, [], WHOLE_RUN)).toBe(summaries);
  });

  it('ignores a malformed log event rather than inventing a step', () => {
    const merged = mergeLogSteps(
      [],
      [
        { createdAt: '2026-07-27T10:00:00Z', payload: null },
        { createdAt: null, payload: { step_id: 'log-a' } },
      ],
      WHOLE_RUN
    );

    expect(merged).toEqual([]);
  });
});
