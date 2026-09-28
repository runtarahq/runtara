import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { StepSummaryResponse } from '@/generated/RuntaraRuntimeApi';
import { toHierarchicalStep } from '@/features/workflows/types/timeline';
import { SUSPENDED_STEP_TITLE } from '@/features/workflows/utils/step-status';
import { ExecutionTimeline } from './index';

const T0 = Date.parse('2026-09-27T20:00:00Z');
const NOW = T0 + 60_000;
const summary = (
  stepId: string,
  offsetMs: number,
  extra: Partial<StepSummaryResponse>
) =>
  toHierarchicalStep(
    {
      stepId,
      stepName: stepId,
      stepType: 'Agent',
      startedAt: new Date(T0 + offsetMs).toISOString(),
      ...extra,
    } as StepSummaryResponse,
    0,
    T0,
    NOW
  );

const timeline = vi.hoisted(() => ({ steps: [] as unknown[] }));
vi.mock('@/features/workflows/hooks/useHierarchicalTimeline', () => ({
  useHierarchicalTimeline: () => ({
    visibleSteps: timeline.steps,
    totalDuration: 60_000,
    stats: { total: 60_000, byType: {}, rootStepCount: 2 },
    isLoadingRoot: false,
    hasMoreRootSteps: false,
  }),
}));
vi.mock('@/shared/hooks', () => ({ useToken: () => 'token' }));
vi.mock('react-oidc-context', () => ({
  useAuth: () => ({ user: { profile: { sub: 'viewer' } } }),
}));
vi.mock('@tanstack/react-query', () => ({
  useQueryClient: () => ({ invalidateQueries: vi.fn() }),
}));
vi.mock('@/shared/hooks/api', () => ({
  useCustomQuery: ({ queryKey }: { queryKey: string[] }) =>
    queryKey.includes('pendingInput')
      ? { data: [], error: null }
      : {
          data: { status: 'suspended', suspensionReason: 'waiting_instances' },
        },
  useCustomMutation: () => ({ mutate: vi.fn() }),
}));
vi.mock('@/features/workflows/queries', () => ({
  getWorkflowInstance: vi.fn(),
  getPendingInput: vi.fn(),
  deliverSignal: vi.fn(),
}));

afterEach(cleanup);

describe('timeline narrow bars', () => {
  // jsdom has no layout; give the time track a real width.
  beforeEach(() => {
    vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockReturnValue(972);
  });
  afterEach(() => vi.restoreAllMocks());

  it('draws the label and badge outside a bar too narrow for them', () => {
    timeline.steps = [
      summary('finance', 0, { status: 'completed', durationMs: 30 }),
      summary('late', 59_990, { status: 'failed', durationMs: 10 }),
      summary('wait', 40, { status: 'suspended', durationMs: null }),
    ];
    const { container } = render(
      <ExecutionTimeline workflowId="workflow" instanceId="instance" />
    );
    const bar = (id: string) =>
      container.querySelector<HTMLElement>(`[data-timeline-bar="${id}"]`)!;
    const outside = (id: string) =>
      container.querySelector<HTMLElement>(`[data-timeline-label="${id}"]`);

    // 30ms of 60s is a 60px minimum-width bar: label goes to its right.
    expect(bar('finance')).toBeEmptyDOMElement();
    expect(outside('finance')).toHaveAttribute('data-label-side', 'right');
    expect(outside('finance')).toHaveTextContent('30msCompleted');
    expect(outside('finance')!.style.left).toBe('66px');

    // A narrow bar at the track end: label goes to its left.
    expect(outside('late')).toHaveAttribute('data-label-side', 'left');
    expect(outside('late')).toHaveTextContent('10msFailed');
    expect(outside('late')!.style.right).toBe('66px');

    // The wide parked bar keeps its label and Suspended badge inside.
    expect(outside('wait')).toBeNull();
    expect(bar('wait')).toHaveTextContent('Suspended');
  });
});

describe('timeline parked step', () => {
  it('draws a parked step as an open-ended bar with the Suspended badge', () => {
    timeline.steps = [
      summary('finance', 0, { status: 'completed', durationMs: 30 }),
      summary('wait', 40, { status: 'suspended', durationMs: null }),
    ];
    const { container } = render(
      <ExecutionTimeline workflowId="workflow" instanceId="instance" />
    );

    const bar = container.querySelector<HTMLElement>(
      '[data-timeline-bar="wait"]'
    );
    expect(bar).not.toBeNull();
    expect(bar).toHaveAttribute('data-open-ended', 'true');
    expect(bar!.className).toContain('[border-right-style:dashed]');
    // Elapsed time so far, not "-".
    expect(bar).toHaveTextContent('59.96s');

    const badge = screen.getByText('Suspended');
    expect(bar).toContainElement(badge);
    expect(badge).toHaveAttribute('title', SUSPENDED_STEP_TITLE);
    expect(badge).toHaveAttribute('data-step-status', 'suspended');

    const done = container.querySelector('[data-timeline-bar="finance"]');
    expect(done).not.toHaveAttribute('data-open-ended');
    expect(done).toHaveTextContent('Completed');
  });
});
