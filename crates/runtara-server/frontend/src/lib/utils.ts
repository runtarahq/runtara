import { type ClassValue, clsx } from 'clsx';
import { twMerge } from 'tailwind-merge';
import { format } from 'date-fns';

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

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
