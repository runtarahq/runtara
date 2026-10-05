import { describe, expect, it } from 'vitest';
import {
  readRunFilters,
  writeRunFilters,
  resolveRunFilters,
  summaryFilters,
  selectedRunStatus,
} from './run-filters';

describe('Runs query context', () => {
  it('round trips both date ranges, sort, parent, label, custom statuses and unrelated URL parameters', () => {
    const query = new URLSearchParams(
      'workflowId=w&parentInstanceId=p&runLabel=ORDER-123&status=queued,cancelled&search=Order&createdFrom=2026-01-01T00:00:00Z&createdTo=2026-02-01T00:00:00Z&completedFrom=2026-01-10T00:00:00Z&completedTo=2026-02-10T00:00:00Z&sortBy=completedAt&sortOrder=asc&range=custom&dateBasis=completed&extra=keep'
    );
    const filters = readRunFilters(query);
    expect(readRunFilters(writeRunFilters(filters, query))).toEqual(filters);
    expect(writeRunFilters(filters, query).get('extra')).toBe('keep');
    expect(selectedRunStatus(filters.status)).toBeUndefined();
    expect(selectedRunStatus('timeout,failed')).toBe('Failed');
    expect(selectedRunStatus()).toBe('All');
  });
  it('resolves relative dates once for matching counts and rows and advances on refresh', () => {
    const now = Date.parse('2026-09-29T12:00:00Z');
    const filters = {
      range: '24h' as const,
      dateBasis: 'completed' as const,
      status: 'failed,timeout',
      workflowId: 'w',
    };
    const resolved = resolveRunFilters(filters, now);
    expect(resolved.completedFrom).toBe('2026-09-28T12:00:00.000Z');
    expect(resolved.createdFrom).toBeUndefined();
    const summary = summaryFilters(resolved);
    expect(summary).not.toHaveProperty('status');
    expect(summary.completedFrom).toBe(resolved.completedFrom);
    expect(summary.completedTo).toBe(resolved.completedTo);
    expect(resolveRunFilters(filters, now + 30_000).completedTo).toBe(
      '2026-09-29T12:00:30.000Z'
    );
  });
  it('does not change dates or sorting when selecting a status', () => {
    const filters = readRunFilters(
      new URLSearchParams(
        'range=7d&dateBasis=started&sortBy=createdAt&sortOrder=asc'
      )
    );
    const next = readRunFilters(
      writeRunFilters({ ...filters, status: 'suspended' })
    );
    expect(next).toEqual({ ...filters, status: 'suspended' });
  });
});
