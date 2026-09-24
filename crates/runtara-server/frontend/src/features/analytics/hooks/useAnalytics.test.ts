import { afterEach, describe, expect, it, vi } from 'vitest';
import { renderHook } from '@testing-library/react';
import { useCustomQuery } from '@/shared/hooks/api';
import { getTenantMetrics } from '../queries';
import { usePreviousTenantMetrics, useTenantMetrics } from './useAnalytics';

vi.mock('@/shared/hooks/api', () => ({ useCustomQuery: vi.fn() }));
vi.mock('../queries', () => ({
  getTenantMetrics: vi.fn().mockResolvedValue({}),
  getSystemAnalytics: vi.fn(),
}));

afterEach(() => {
  vi.useRealTimers();
  vi.clearAllMocks();
});

describe('Usage refresh bounds', () => {
  it('advances current and previous windows on refetch, aligned to minutes', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-09-24T12:00:34Z'));
    renderHook(() => useTenantMetrics('1h'));
    renderHook(() => usePreviousTenantMetrics('1h'));
    const [current, previous] = vi
      .mocked(useCustomQuery)
      .mock.calls.map(([options]) => options);
    await current.queryFn('test-token');
    await previous.queryFn('test-token');
    expect(getTenantMetrics).toHaveBeenNthCalledWith(
      1,
      'test-token',
      '2026-09-24T11:00:00.000Z',
      '2026-09-24T12:00:00.000Z',
      '1m'
    );
    expect(getTenantMetrics).toHaveBeenNthCalledWith(
      2,
      'test-token',
      '2026-09-24T10:00:00.000Z',
      '2026-09-24T11:00:00.000Z',
      '1h'
    );
    vi.setSystemTime(new Date('2026-09-24T12:02:55Z'));
    await current.queryFn('test-token');
    await previous.queryFn('test-token');
    expect(getTenantMetrics).toHaveBeenNthCalledWith(
      3,
      'test-token',
      '2026-09-24T11:02:00.000Z',
      '2026-09-24T12:02:00.000Z',
      '1m'
    );
    expect(getTenantMetrics).toHaveBeenNthCalledWith(
      4,
      'test-token',
      '2026-09-24T10:02:00.000Z',
      '2026-09-24T11:02:00.000Z',
      '1h'
    );
  });
});
