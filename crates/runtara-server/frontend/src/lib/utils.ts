import { type ClassValue, clsx } from 'clsx';
import { twMerge } from 'tailwind-merge';
import { format } from 'date-fns';

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

/**
 * The canonical absolute timestamp for the console — "18 Jul, 2026 8:29 AM".
 * Every list/table date cell renders through this so comparable screens agree;
 * reach for `toLocaleString` and you get a US-locale string with seconds that
 * matches nothing else in the app.
 */
export function formatDate(
  date: Date | string | undefined,
  pattern: string = 'dd MMM, yyyy p'
) {
  // Handle invalid date inputs
  if (!date) {
    return 'Invalid date';
  }

  try {
    const dateObj = new Date(date);
    // Check if date is valid
    if (isNaN(dateObj.getTime())) {
      return 'Invalid date';
    }
    return format(dateObj, pattern);
  } catch (error) {
    console.error('Error formatting date:', error);
    return 'Invalid date';
  }
}

const MINUTE_MS = 60_000;
const HOUR_MS = 60 * MINUTE_MS;
const DAY_MS = 24 * HOUR_MS;
const RELATIVE_CUTOFF_MS = 7 * DAY_MS;

/**
 * Recency for surfaces where "how long ago" beats a wall-clock reading —
 * "just now", "5 min ago", "3 hr ago", "2 days ago". Past a week the relative
 * phrasing stops being useful, so it falls back to {@link formatDate} rather
 * than to a locale string, keeping the older half of a list consistent with
 * every other timestamp on screen.
 */
export function formatRelativeTime(date: Date | string | undefined): string {
  if (!date) {
    return 'Invalid date';
  }

  const dateObj = new Date(date);
  if (isNaN(dateObj.getTime())) {
    return 'Invalid date';
  }

  const diffMs = Date.now() - dateObj.getTime();
  if (diffMs >= RELATIVE_CUTOFF_MS) {
    return formatDate(dateObj);
  }

  const diffMins = Math.floor(diffMs / MINUTE_MS);
  if (diffMins < 1) return 'just now';
  if (diffMins < 60) return `${diffMins} min ago`;

  const diffHours = Math.floor(diffMs / HOUR_MS);
  if (diffHours < 24) return `${diffHours} hr ago`;

  return `${Math.floor(diffMs / DAY_MS)} days ago`;
}

export const range = (start: number, end?: number, step = 1) => {
  const output = [];

  if (typeof end === 'undefined') {
    end = start;
    start = 0;
  }

  for (let i = start; i < end; i += step) {
    output.push(i);
  }

  return output;
};

export const checkUserGroup = (
  allowedGroups: string[],
  userGroups: string[]
): boolean => {
  if (!allowedGroups.length) {
    return true;
  }

  return allowedGroups.some((group) => userGroups.includes(group));
};

/**
 * Cleans up pointer-events style on document.body
 * This can be called from anywhere to ensure UI elements remain clickable
 */
export const cleanupPointerEvents = () => {
  if (typeof document !== 'undefined') {
    document.body.style.removeProperty('pointer-events');
  }
};
