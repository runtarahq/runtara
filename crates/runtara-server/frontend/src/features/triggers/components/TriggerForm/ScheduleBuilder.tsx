import { useState, useEffect, useCallback } from 'react';
import { PlusIcon, Cross2Icon } from '@radix-ui/react-icons';
import { cn } from '@/lib/utils';
import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import { FormLabel } from '@/shared/components/ui/form';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import {
  type ScheduleConfig,
  type ScheduleType,
  type IntervalUnit,
  DAYS_OF_WEEK,
  getScheduleDescription,
  isValidCronExpression,
} from '@/features/triggers/utils/cron';

interface ScheduleBuilderProps {
  value: ScheduleConfig;
  onChange: (config: ScheduleConfig) => void;
  disabled?: boolean;
}

const SCHEDULE_TYPE_OPTIONS: {
  value: ScheduleType;
  label: string;
  description: string;
}[] = [
  {
    value: 'interval',
    label: 'Interval',
    description: 'Run every X minutes/hours/days',
  },
  {
    value: 'daily',
    label: 'Daily',
    description: 'Run at specific time(s) every day',
  },
  {
    value: 'weekly',
    label: 'Weekly',
    description: 'Run on specific days of the week',
  },
  {
    value: 'monthly',
    label: 'Monthly',
    description: 'Run on specific days of the month',
  },
  {
    value: 'custom',
    label: 'Custom',
    description: 'Enter a custom cron expression',
  },
];

const INTERVAL_UNIT_OPTIONS: { value: IntervalUnit; label: string }[] = [
  { value: 'minutes', label: 'minutes' },
  { value: 'hours', label: 'hours' },
  { value: 'days', label: 'days' },
  { value: 'months', label: 'months' },
];

export function ScheduleBuilder({
  value,
  onChange,
  disabled,
}: ScheduleBuilderProps) {
  const [customExpressionError, setCustomExpressionError] = useState<
    string | null
  >(null);

  const handleTypeChange = useCallback(
    (newType: ScheduleType) => {
      const newConfig: ScheduleConfig = { type: newType };

      switch (newType) {
        case 'interval':
          newConfig.intervalValue = value.intervalValue || 5;
          newConfig.intervalUnit = value.intervalUnit || 'minutes';
          break;
        case 'daily':
          newConfig.times = value.times?.length
            ? value.times
            : [{ hour: 9, minute: 0 }];
          break;
        case 'weekly':
          newConfig.times = value.times?.length
            ? value.times
            : [{ hour: 9, minute: 0 }];
          newConfig.daysOfWeek = value.daysOfWeek?.length
            ? value.daysOfWeek
            : [1]; // Monday
          break;
        case 'monthly':
          newConfig.times = value.times?.length
            ? value.times
            : [{ hour: 9, minute: 0 }];
          newConfig.daysOfMonth = value.daysOfMonth?.length
            ? value.daysOfMonth
            : [1];
          break;
        case 'custom':
          newConfig.customExpression = value.customExpression || '0 9 * * *';
          break;
      }

      onChange(newConfig);
    },
    [value, onChange]
  );

  const handleIntervalValueChange = useCallback(
    (newValue: number) => {
      onChange({
        ...value,
        intervalValue: Math.max(1, newValue),
      });
    },
    [value, onChange]
  );

  const handleIntervalUnitChange = useCallback(
    (newUnit: IntervalUnit) => {
      onChange({
        ...value,
        intervalUnit: newUnit,
      });
    },
    [value, onChange]
  );

  const handleTimeChange = useCallback(
    (index: number, field: 'hour' | 'minute', newValue: number) => {
      const times = [...(value.times || [{ hour: 9, minute: 0 }])];
      times[index] = {
        ...times[index],
        [field]:
          field === 'hour'
            ? Math.min(23, Math.max(0, newValue))
            : Math.min(59, Math.max(0, newValue)),
      };
      onChange({
        ...value,
        times,
      });
    },
    [value, onChange]
  );

  const handleAddTime = useCallback(() => {
    const times = [...(value.times || [{ hour: 9, minute: 0 }])];
    // Add a new time 1 hour after the last one
    const lastTime = times[times.length - 1];
    const newHour = (lastTime.hour + 1) % 24;
    times.push({ hour: newHour, minute: lastTime.minute });
    onChange({
      ...value,
      times,
    });
  }, [value, onChange]);

  const handleRemoveTime = useCallback(
    (index: number) => {
      const times = [...(value.times || [])];
      if (times.length > 1) {
        times.splice(index, 1);
        onChange({
          ...value,
          times,
        });
      }
    },
    [value, onChange]
  );

  const handleDayOfWeekToggle = useCallback(
    (day: number) => {
      const days = [...(value.daysOfWeek || [])];
      const index = days.indexOf(day);
      if (index >= 0) {
        if (days.length > 1) {
          days.splice(index, 1);
        }
      } else {
        days.push(day);
      }
      onChange({
        ...value,
        daysOfWeek: days.sort((a, b) => a - b),
      });
    },
    [value, onChange]
  );

  const handleDayOfMonthToggle = useCallback(
    (day: number) => {
      const days = [...(value.daysOfMonth || [])];
      const index = days.indexOf(day);
      if (index >= 0) {
        if (days.length > 1) {
          days.splice(index, 1);
        }
      } else {
        days.push(day);
      }
      onChange({
        ...value,
        daysOfMonth: days.sort((a, b) => a - b),
      });
    },
    [value, onChange]
  );

  const handleCustomExpressionChange = useCallback(
    (expression: string) => {
      if (expression && !isValidCronExpression(expression)) {
        setCustomExpressionError(
          'Invalid cron expression. Format: minute hour day-of-month month day-of-week'
        );
      } else {
        setCustomExpressionError(null);
      }
      onChange({
        ...value,
        customExpression: expression,
      });
    },
    [value, onChange]
  );

  // Validate custom expression on mount
  useEffect(() => {
    if (value.type === 'custom' && value.customExpression) {
      if (!isValidCronExpression(value.customExpression)) {
        setCustomExpressionError('Invalid cron expression');
      } else {
        setCustomExpressionError(null);
      }
    }
  }, [value.type, value.customExpression]);

  return (
    <div className="space-y-4">
      {/* Schedule Type Selector */}
      <div className="space-y-2">
        <FormLabel>Repeat</FormLabel>
        <Select
          value={value.type}
          onValueChange={(v) => handleTypeChange(v as ScheduleType)}
          disabled={disabled}
        >
          <SelectTrigger className="h-auto min-h-10 w-full py-2">
            <SelectValue placeholder="Select schedule type">
              {value.type && (
                <div className="flex flex-col items-start gap-0.5">
                  <span>
                    {
                      SCHEDULE_TYPE_OPTIONS.find((o) => o.value === value.type)
                        ?.label
                    }
                  </span>
                  <span className="text-xs text-muted-foreground">
                    {
                      SCHEDULE_TYPE_OPTIONS.find((o) => o.value === value.type)
                        ?.description
                    }
                  </span>
                </div>
              )}
            </SelectValue>
          </SelectTrigger>
          <SelectContent>
            {SCHEDULE_TYPE_OPTIONS.map((option) => (
              <SelectItem
                key={option.value}
                value={option.value}
                className="py-2"
              >
                <div className="flex flex-col items-start gap-0.5">
                  <span>{option.label}</span>
                  <span className="text-xs text-muted-foreground">
                    {option.description}
                  </span>
                </div>
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      {/* Interval Configuration */}
      {value.type === 'interval' && (
        <div className="space-y-2">
          <FormLabel>Every</FormLabel>
          <div className="flex items-center gap-2">
            <Input
              type="number"
              min={1}
              value={value.intervalValue || 5}
              onChange={(e) =>
                handleIntervalValueChange(parseInt(e.target.value) || 1)
              }
              className="w-24"
              disabled={disabled}
            />
            <Select
              value={value.intervalUnit || 'minutes'}
              onValueChange={(v) => handleIntervalUnitChange(v as IntervalUnit)}
              disabled={disabled}
            >
              <SelectTrigger className="w-32">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {INTERVAL_UNIT_OPTIONS.map((option) => (
                  <SelectItem key={option.value} value={option.value}>
                    {option.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        </div>
      )}

      {/* Time Slots for Daily/Weekly/Monthly */}
      {(value.type === 'daily' ||
        value.type === 'weekly' ||
        value.type === 'monthly') && (
        <div className="space-y-3">
          <div className="flex items-center justify-between">
            <FormLabel>
              Time{(value.times?.length || 0) > 1 ? 's' : ''}
            </FormLabel>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={handleAddTime}
              disabled={disabled || (value.times?.length || 0) >= 5}
              className="h-7 px-2 text-xs"
            >
              <PlusIcon className="mr-1 h-3 w-3" />
              Add time
            </Button>
          </div>
          <div className="space-y-2">
            {(value.times || [{ hour: 9, minute: 0 }]).map((time, index) => (
              <div key={index} className="flex items-center gap-2">
                <div className="flex items-center gap-1">
                  <Input
                    type="number"
                    min={0}
                    max={23}
                    value={time.hour.toString().padStart(2, '0')}
                    onChange={(e) =>
                      handleTimeChange(
                        index,
                        'hour',
                        parseInt(e.target.value) || 0
                      )
                    }
                    className="w-16 text-center"
                    disabled={disabled}
                  />
                  <span className="text-muted-foreground">:</span>
                  <Input
                    type="number"
                    min={0}
                    max={59}
                    value={time.minute.toString().padStart(2, '0')}
                    onChange={(e) =>
                      handleTimeChange(
                        index,
                        'minute',
                        parseInt(e.target.value) || 0
                      )
                    }
                    className="w-16 text-center"
                    disabled={disabled}
                  />
                </div>
                {(value.times?.length || 0) > 1 && (
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    onClick={() => handleRemoveTime(index)}
                    disabled={disabled}
                    className="h-7 w-7"
                  >
                    <Cross2Icon className="h-3 w-3" />
                  </Button>
                )}
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Days of Week for Weekly */}
      {value.type === 'weekly' && (
        <div className="space-y-2">
          <FormLabel>On</FormLabel>
          <div className="flex flex-wrap gap-1.5">
            {DAYS_OF_WEEK.map((day) => {
              const isSelected = value.daysOfWeek?.includes(day.value);
              return (
                <button
                  key={day.value}
                  type="button"
                  onClick={() => handleDayOfWeekToggle(day.value)}
                  disabled={disabled}
                  className={cn(
                    'flex h-9 w-9 items-center justify-center rounded-full text-sm font-medium transition-colors',
                    'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2',
                    isSelected
                      ? 'bg-primary text-primary-foreground'
                      : 'bg-muted text-muted-foreground hover:bg-muted/80',
                    disabled && 'cursor-not-allowed opacity-50'
                  )}
                  title={day.fullLabel}
                >
                  {day.label.charAt(0)}
                </button>
              );
            })}
          </div>
          <div className="text-xs text-muted-foreground">
            Selected:{' '}
            {value.daysOfWeek?.map((d) => DAYS_OF_WEEK[d]?.label).join(', ') ||
              'None'}
          </div>
        </div>
      )}

      {/* Days of Month for Monthly */}
      {value.type === 'monthly' && (
        <div className="space-y-2">
          <FormLabel>On day(s) of month</FormLabel>
          <div className="grid grid-cols-7 gap-1">
            {Array.from({ length: 31 }, (_, i) => i + 1).map((day) => {
              const isSelected = value.daysOfMonth?.includes(day);
              return (
                <button
                  key={day}
                  type="button"
                  onClick={() => handleDayOfMonthToggle(day)}
                  disabled={disabled}
                  className={cn(
                    'flex h-8 w-8 items-center justify-center rounded text-xs font-medium transition-colors',
                    'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1',
                    isSelected
                      ? 'bg-primary text-primary-foreground'
                      : 'bg-muted text-muted-foreground hover:bg-muted/80',
                    disabled && 'cursor-not-allowed opacity-50'
                  )}
                >
                  {day}
                </button>
              );
            })}
          </div>
          <div className="text-xs text-muted-foreground">
            Selected: {value.daysOfMonth?.join(', ') || 'None'}
          </div>
        </div>
      )}

      {/* Custom Cron Expression */}
      {value.type === 'custom' && (
        <div className="space-y-2">
          <FormLabel>Cron Expression</FormLabel>
          <Input
            type="text"
            value={value.customExpression || ''}
            onChange={(e) => handleCustomExpressionChange(e.target.value)}
            placeholder="0 9 * * *"
            disabled={disabled}
            className={cn(customExpressionError && 'border-destructive')}
          />
          <div className="text-xs text-muted-foreground">
            Format: minute hour day-of-month month day-of-week
          </div>
          {customExpressionError && (
            <div className="text-xs text-destructive">
              {customExpressionError}
            </div>
          )}
          <div className="rounded-md bg-muted p-2">
            <div className="text-xs text-muted-foreground">
              <strong>Examples:</strong>
              <ul className="mt-1 space-y-0.5">
                <li>
                  <code className="text-foreground">0 9 * * *</code> - Every day
                  at 9:00 AM
                </li>
                <li>
                  <code className="text-foreground">0 9 * * 1-5</code> -
                  Weekdays at 9:00 AM
                </li>
                <li>
                  <code className="text-foreground">0 9,18 * * *</code> - Daily
                  at 9:00 AM and 6:00 PM
                </li>
                <li>
                  <code className="text-foreground">0 0 1 * *</code> - First day
                  of every month at midnight
                </li>
              </ul>
            </div>
          </div>
        </div>
      )}

      {/* Schedule Summary */}
      <div className="rounded-md bg-muted/50 p-3">
        <div className="text-sm">
          <span className="font-medium">Schedule: </span>
          <span className="text-muted-foreground">
            {getScheduleDescription(value)}
          </span>
        </div>
      </div>
    </div>
  );
}
