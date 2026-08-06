import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { formatDate, formatRelativeTime } from './utils.ts';

/** A fixed "now" so relative-time buckets are deterministic. */
const NOW = new Date('2026-07-27T12:00:00');

const minutesAgo = (n: number) => new Date(NOW.getTime() - n * 60_000);
const hoursAgo = (n: number) => new Date(NOW.getTime() - n * 3_600_000);
const daysAgo = (n: number) => new Date(NOW.getTime() - n * 86_400_000);

describe('Date Utilities', () => {
  describe('formatDate', () => {
    it('should use the canonical console pattern by default', () => {
      expect(formatDate(new Date('2026-07-18T08:29:00'))).toBe(
        '18 Jul, 2026 8:29 AM'
      );
    });

    it('should accept an ISO string', () => {
      expect(formatDate('2026-07-22T12:10:00')).toBe('22 Jul, 2026 12:10 PM');
    });

    it('should honour an explicit pattern', () => {
      expect(formatDate('2026-07-22T12:10:00', 'dd MMM, yyyy')).toBe(
        '22 Jul, 2026'
      );
    });

    it('should return a sentinel for a missing date', () => {
      expect(formatDate(undefined)).toBe('Invalid date');
    });

    it('should return a sentinel for an unparseable date', () => {
      expect(formatDate('not-a-date')).toBe('Invalid date');
    });
  });

  describe('formatRelativeTime', () => {
    beforeEach(() => {
      vi.useFakeTimers();
      vi.setSystemTime(NOW);
    });

    afterEach(() => {
      vi.useRealTimers();
    });

    it('should report sub-minute ages as "just now"', () => {
      expect(formatRelativeTime(new Date(NOW.getTime() - 30_000))).toBe(
        'just now'
      );
    });

    it('should report minutes', () => {
      expect(formatRelativeTime(minutesAgo(1))).toBe('1 min ago');
      expect(formatRelativeTime(minutesAgo(59))).toBe('59 min ago');
    });

    it('should report hours', () => {
      expect(formatRelativeTime(hoursAgo(1))).toBe('1 hr ago');
      expect(formatRelativeTime(hoursAgo(23))).toBe('23 hr ago');
    });

    it('should report days', () => {
      expect(formatRelativeTime(daysAgo(1))).toBe('1 days ago');
      expect(formatRelativeTime(daysAgo(6))).toBe('6 days ago');
    });

    it('should fall back to the canonical absolute format past a week', () => {
      // Not a locale string — the older half of a list has to match every
      // other timestamp on screen.
      expect(formatRelativeTime(daysAgo(7))).toBe('20 Jul, 2026 12:00 PM');
      expect(formatRelativeTime(daysAgo(400))).toBe('22 Jun, 2025 12:00 PM');
    });

    it('should treat a future timestamp as "just now"', () => {
      expect(formatRelativeTime(new Date(NOW.getTime() + 60_000))).toBe(
        'just now'
      );
    });

    it('should return a sentinel for a missing date', () => {
      expect(formatRelativeTime(undefined)).toBe('Invalid date');
    });

    it('should return a sentinel for an unparseable date', () => {
      expect(formatRelativeTime('not-a-date')).toBe('Invalid date');
    });
  });
});
