import { afterEach, describe, expect, it, vi } from 'vitest';

import { holdForQueuedRow, queuedInstancePlaceholder } from './index';

/**
 * Reading a just-queued run 404s: `execute` returns an instance id before the
 * runtime has written the row. `holdForQueuedRow` waits out that window — but
 * only for the run the caller queued, since every other read (attaching to an
 * existing instance, revisiting a finished one) must stay immediate.
 */
describe('holdForQueuedRow', () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it('does not wait when nothing was queued', async () => {
    vi.useFakeTimers();
    let settled = false;
    void holdForQueuedRow(null, 'inst_1').then(() => {
      settled = true;
    });

    await vi.advanceTimersByTimeAsync(0);
    expect(settled).toBe(true);
  });

  it('does not wait for an instance other than the queued one', async () => {
    vi.useFakeTimers();
    const queued = { instanceId: 'inst_1', queuedAt: Date.now() };
    let settled = false;
    void holdForQueuedRow(queued, 'inst_2').then(() => {
      settled = true;
    });

    await vi.advanceTimersByTimeAsync(0);
    expect(settled).toBe(true);
  });

  it('holds the queued instance until the grace window has passed', async () => {
    vi.useFakeTimers();
    const queued = { instanceId: 'inst_1', queuedAt: Date.now() };
    let settled = false;
    void holdForQueuedRow(queued, 'inst_1').then(() => {
      settled = true;
    });

    await vi.advanceTimersByTimeAsync(500);
    expect(settled).toBe(false);

    await vi.advanceTimersByTimeAsync(1000);
    expect(settled).toBe(true);
  });

  it('does not wait once the grace window has already elapsed', async () => {
    vi.useFakeTimers();
    // A poll several seconds into the run must not be delayed again.
    const queued = { instanceId: 'inst_1', queuedAt: Date.now() - 10_000 };
    let settled = false;
    void holdForQueuedRow(queued, 'inst_1').then(() => {
      settled = true;
    });

    await vi.advanceTimersByTimeAsync(0);
    expect(settled).toBe(true);
  });
});

describe('queuedInstancePlaceholder', () => {
  it('reports the run as queued under the id execute handed back', () => {
    const placeholder = queuedInstancePlaceholder('wf_1', 'inst_1', 3);

    expect(placeholder.id).toBe('inst_1');
    expect(placeholder.workflowId).toBe('wf_1');
    expect(placeholder.status).toBe('queued');
    expect(placeholder.usedVersion).toBe(3);
  });
});
