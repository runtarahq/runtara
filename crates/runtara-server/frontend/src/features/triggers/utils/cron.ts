// Standard 5-field cron format: minute hour day-of-month month day-of-week
// Note: Seconds are NOT supported - minimum granularity is 1 minute

// Schedule types for the enhanced scheduler
export type ScheduleType =
  | 'interval'
  | 'daily'
  | 'weekly'
  | 'monthly'
  | 'custom';

export type IntervalUnit = 'minutes' | 'hours' | 'days' | 'months';

export interface TimeSlot {
  hour: number;
  minute: number;
}

export interface ScheduleConfig {
  type: ScheduleType;
  // For interval type
  intervalValue?: number;
  intervalUnit?: IntervalUnit;
  // For daily/weekly/monthly types
  times?: TimeSlot[];
  // For weekly type
  daysOfWeek?: number[]; // 0-6, Sunday=0
  // For monthly type
  daysOfMonth?: number[]; // 1-31
  // For custom type
  customExpression?: string;
}

// Day names for display
export const DAYS_OF_WEEK = [
  { value: 0, label: 'Sun', fullLabel: 'Sunday' },
  { value: 1, label: 'Mon', fullLabel: 'Monday' },
  { value: 2, label: 'Tue', fullLabel: 'Tuesday' },
  { value: 3, label: 'Wed', fullLabel: 'Wednesday' },
  { value: 4, label: 'Thu', fullLabel: 'Thursday' },
  { value: 5, label: 'Fri', fullLabel: 'Friday' },
  { value: 6, label: 'Sat', fullLabel: 'Saturday' },
];

// ============================================================================
// Legacy functions (for backwards compatibility)
// ============================================================================

const minuteRegex = /^\*\/[0-5]?[0-9]$/;
const hourRegex = /^\*\/([0-9]|1[0-9]|2[0-3])$/;
const dayRegex = /^\*\/([1-9]|[12][0-9]|3[01])$/;
const monthRegex = /^\*\/([1-9]|1[0-2])$/;

const humanToCron = (value: number, unit: string) => {
  switch (unit) {
    case 'minutes':
      return `*/${value} * * * *`;
    case 'hours':
      return `0 */${value} * * *`;
    case 'days':
      return `0 0 */${value} * *`;
    case 'months':
      return `0 0 1 */${value} *`;
    default:
      return '* * * * *';
  }
};

export function cronToHuman(cronExpression: string) {
  if (!cronExpression.includes('*')) {
    return {};
  }

  const cronFields = cronExpression.split(' ');
  const [minute, hour, dayOfMonth, months] = cronFields;

  if (minuteRegex.test(minute)) {
    return {
      time: parseInt(minute.replace(/\D/g, '')),
      timeUnit: 'minutes',
    };
  } else if (hourRegex.test(hour)) {
    return {
      time: parseInt(hour.replace(/\D/g, '')),
      timeUnit: 'hours',
    };
  } else if (/^0$/.test(hour) && dayRegex.test(dayOfMonth)) {
    return {
      time: parseInt(dayOfMonth.replace(/\D/g, '')),
      timeUnit: 'days',
    };
  } else if (/^1$/.test(dayOfMonth) && monthRegex.test(months)) {
    return {
      time: parseInt(months.replace(/\D/g, '')),
      timeUnit: 'months',
    };
  } else {
    return {
      time: 1,
      timeUnit: 'minutes',
    };
  }
}

// ============================================================================
// Enhanced cron conversion functions
// ============================================================================

/**
 * Convert a ScheduleConfig to a cron expression
 */
export function scheduleToCron(config: ScheduleConfig): string {
  switch (config.type) {
    case 'interval':
      return humanToCron(
        config.intervalValue || 5,
        config.intervalUnit || 'minutes'
      );

    case 'daily': {
      // Daily at specific times
      // For multiple times, we need to generate multiple minute/hour values
      const times = config.times || [{ hour: 9, minute: 0 }];
      if (times.length === 1) {
        return `${times[0].minute} ${times[0].hour} * * *`;
      }
      // Multiple times: group by same minute, then by same hour
      const minutes = [...new Set(times.map((t) => t.minute))].sort(
        (a, b) => a - b
      );
      const hours = [...new Set(times.map((t) => t.hour))].sort(
        (a, b) => a - b
      );

      // If all times have the same minute, we can use comma-separated hours
      if (minutes.length === 1) {
        return `${minutes[0]} ${hours.join(',')} * * *`;
      }
      // Otherwise, we use comma-separated for both (this may trigger more than intended)
      // For exact control, we'd need multiple cron expressions, but standard cron only supports one
      // We'll use the first time as primary
      return `${times[0].minute} ${times[0].hour} * * *`;
    }

    case 'weekly': {
      // Weekly on specific days at specific times
      const times = config.times || [{ hour: 9, minute: 0 }];
      const days = config.daysOfWeek || [1]; // Default to Monday
      const daysStr = days.sort((a, b) => a - b).join(',');

      if (times.length === 1) {
        return `${times[0].minute} ${times[0].hour} * * ${daysStr}`;
      }
      // Multiple times on specific days
      const hours = [...new Set(times.map((t) => t.hour))].sort(
        (a, b) => a - b
      );
      const minutes = [...new Set(times.map((t) => t.minute))].sort(
        (a, b) => a - b
      );

      if (minutes.length === 1) {
        return `${minutes[0]} ${hours.join(',')} * * ${daysStr}`;
      }
      return `${times[0].minute} ${times[0].hour} * * ${daysStr}`;
    }

    case 'monthly': {
      // Monthly on specific days at specific times
      const times = config.times || [{ hour: 9, minute: 0 }];
      const days = config.daysOfMonth || [1]; // Default to 1st of month
      const daysStr = days.sort((a, b) => a - b).join(',');

      if (times.length === 1) {
        return `${times[0].minute} ${times[0].hour} ${daysStr} * *`;
      }
      // Multiple times on specific days
      const hours = [...new Set(times.map((t) => t.hour))].sort(
        (a, b) => a - b
      );
      const minutes = [...new Set(times.map((t) => t.minute))].sort(
        (a, b) => a - b
      );

      if (minutes.length === 1) {
        return `${minutes[0]} ${hours.join(',')} ${daysStr} * *`;
      }
      return `${times[0].minute} ${times[0].hour} ${daysStr} * *`;
    }

    case 'custom':
      return config.customExpression || '0 * * * *';

    default:
      return '0 * * * *';
  }
}

/**
 * Parse a cron expression into a ScheduleConfig
 */
export function cronToSchedule(cronExpression: string): ScheduleConfig {
  if (!cronExpression) {
    return {
      type: 'interval',
      intervalValue: 5,
      intervalUnit: 'minutes',
    };
  }

  const parts = cronExpression.trim().split(/\s+/);
  if (parts.length !== 5) {
    return {
      type: 'custom',
      customExpression: cronExpression,
    };
  }

  const [minute, hour, dayOfMonth, month, dayOfWeek] = parts;

  // Check for interval patterns first (legacy support)
  if (
    minuteRegex.test(minute) &&
    hour === '*' &&
    dayOfMonth === '*' &&
    month === '*' &&
    dayOfWeek === '*'
  ) {
    return {
      type: 'interval',
      intervalValue: parseInt(minute.replace(/\D/g, '')),
      intervalUnit: 'minutes',
    };
  }

  if (
    minute === '0' &&
    hourRegex.test(hour) &&
    dayOfMonth === '*' &&
    month === '*' &&
    dayOfWeek === '*'
  ) {
    return {
      type: 'interval',
      intervalValue: parseInt(hour.replace(/\D/g, '')),
      intervalUnit: 'hours',
    };
  }

  if (
    minute === '0' &&
    hour === '0' &&
    dayRegex.test(dayOfMonth) &&
    month === '*' &&
    dayOfWeek === '*'
  ) {
    return {
      type: 'interval',
      intervalValue: parseInt(dayOfMonth.replace(/\D/g, '')),
      intervalUnit: 'days',
    };
  }

  if (
    minute === '0' &&
    hour === '0' &&
    dayOfMonth === '1' &&
    monthRegex.test(month) &&
    dayOfWeek === '*'
  ) {
    return {
      type: 'interval',
      intervalValue: parseInt(month.replace(/\D/g, '')),
      intervalUnit: 'months',
    };
  }

  // Parse specific times
  const parseTimeValues = (minuteStr: string, hourStr: string): TimeSlot[] => {
    const minutes = minuteStr
      .split(',')
      .map((m) => parseInt(m))
      .filter((m) => !isNaN(m));
    const hours = hourStr
      .split(',')
      .map((h) => parseInt(h))
      .filter((h) => !isNaN(h));

    const times: TimeSlot[] = [];
    for (const h of hours) {
      for (const m of minutes) {
        times.push({ hour: h, minute: m });
      }
    }
    return times.length > 0 ? times : [{ hour: 9, minute: 0 }];
  };

  // Check for weekly pattern (specific day of week, any day of month)
  if (dayOfMonth === '*' && month === '*' && dayOfWeek !== '*') {
    const days = dayOfWeek
      .split(',')
      .map((d) => parseInt(d))
      .filter((d) => !isNaN(d) && d >= 0 && d <= 6);
    return {
      type: 'weekly',
      times: parseTimeValues(minute, hour),
      daysOfWeek: days.length > 0 ? days : [1],
    };
  }

  // Check for monthly pattern (specific day of month, any day of week)
  if (dayOfMonth !== '*' && month === '*' && dayOfWeek === '*') {
    const days = dayOfMonth
      .split(',')
      .map((d) => parseInt(d))
      .filter((d) => !isNaN(d) && d >= 1 && d <= 31);
    return {
      type: 'monthly',
      times: parseTimeValues(minute, hour),
      daysOfMonth: days.length > 0 ? days : [1],
    };
  }

  // Check for daily pattern (any day of month, any day of week, specific time)
  if (
    dayOfMonth === '*' &&
    month === '*' &&
    dayOfWeek === '*' &&
    !minute.includes('/') &&
    !hour.includes('/')
  ) {
    return {
      type: 'daily',
      times: parseTimeValues(minute, hour),
    };
  }

  // Fallback to custom
  return {
    type: 'custom',
    customExpression: cronExpression,
  };
}

/**
 * Get a human-readable description of a schedule
 */
export function getScheduleDescription(config: ScheduleConfig): string {
  const formatTime = (time: TimeSlot): string => {
    const h = time.hour.toString().padStart(2, '0');
    const m = time.minute.toString().padStart(2, '0');
    return `${h}:${m}`;
  };

  const formatTimes = (times: TimeSlot[]): string => {
    if (times.length === 0) return 'at 09:00';
    if (times.length === 1) return `at ${formatTime(times[0])}`;
    if (times.length === 2)
      return `at ${formatTime(times[0])} and ${formatTime(times[1])}`;
    const last = times[times.length - 1];
    const rest = times.slice(0, -1);
    return `at ${rest.map(formatTime).join(', ')}, and ${formatTime(last)}`;
  };

  switch (config.type) {
    case 'interval': {
      const value = config.intervalValue || 5;
      const unit = config.intervalUnit || 'minutes';
      if (value === 1) {
        return `Every ${unit.slice(0, -1)}`; // Remove 's' for singular
      }
      return `Every ${value} ${unit}`;
    }

    case 'daily': {
      const times = config.times || [{ hour: 9, minute: 0 }];
      if (times.length === 1) {
        return `Daily ${formatTimes(times)}`;
      }
      return `${times.length} times daily ${formatTimes(times)}`;
    }

    case 'weekly': {
      const times = config.times || [{ hour: 9, minute: 0 }];
      const days = config.daysOfWeek || [1];
      const dayNames = days
        .map((d) => DAYS_OF_WEEK[d]?.label || 'Unknown')
        .join(', ');
      return `Weekly on ${dayNames} ${formatTimes(times)}`;
    }

    case 'monthly': {
      const times = config.times || [{ hour: 9, minute: 0 }];
      const days = config.daysOfMonth || [1];
      const dayStr =
        days.length === 1 ? `day ${days[0]}` : `days ${days.join(', ')}`;
      return `Monthly on ${dayStr} ${formatTimes(times)}`;
    }

    case 'custom':
      return `Custom: ${config.customExpression || ''}`;

    default:
      return 'Unknown schedule';
  }
}

/**
 * Validate a cron expression
 */
export function isValidCronExpression(expression: string): boolean {
  if (!expression) return false;

  const parts = expression.trim().split(/\s+/);
  if (parts.length !== 5) return false;

  const [minute, hour, dayOfMonth, month, dayOfWeek] = parts;

  // Simple validation patterns
  const fieldPattern = /^(\*|(\d+(-\d+)?(,\d+(-\d+)?)*)|(\*\/\d+))$/;

  const isValidMinute =
    fieldPattern.test(minute) &&
    (minute === '*' ||
      minute.startsWith('*/') ||
      minute.split(',').every((v) => parseInt(v) >= 0 && parseInt(v) <= 59));

  const isValidHour =
    fieldPattern.test(hour) &&
    (hour === '*' ||
      hour.startsWith('*/') ||
      hour.split(',').every((v) => parseInt(v) >= 0 && parseInt(v) <= 23));

  const isValidDayOfMonth =
    fieldPattern.test(dayOfMonth) &&
    (dayOfMonth === '*' ||
      dayOfMonth.startsWith('*/') ||
      dayOfMonth
        .split(',')
        .every((v) => parseInt(v) >= 1 && parseInt(v) <= 31));

  const isValidMonth =
    fieldPattern.test(month) &&
    (month === '*' ||
      month.startsWith('*/') ||
      month.split(',').every((v) => parseInt(v) >= 1 && parseInt(v) <= 12));

  const isValidDayOfWeek =
    fieldPattern.test(dayOfWeek) &&
    (dayOfWeek === '*' ||
      dayOfWeek.startsWith('*/') ||
      dayOfWeek.split(',').every((v) => parseInt(v) >= 0 && parseInt(v) <= 6));

  return (
    isValidMinute &&
    isValidHour &&
    isValidDayOfMonth &&
    isValidMonth &&
    isValidDayOfWeek
  );
}
