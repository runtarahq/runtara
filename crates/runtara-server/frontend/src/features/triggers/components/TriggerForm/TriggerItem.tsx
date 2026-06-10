import { z } from 'zod';
import { WorkflowField } from './WorkflowField';
import { ScheduleField } from './ScheduleField';
import { ConfigurationField } from './ConfigurationField';
import { ChannelConnectionField } from './ChannelConnectionField';
import { WebhookConnectionField } from './WebhookConnectionField';
import { CronInputsField } from './CronInputsField';
import { type ScheduleConfig } from '@/features/triggers/utils/cron';
import {
  isAcceptedCronExpression,
  staticInputsError,
} from '@/features/triggers/utils/trigger-configuration';

// Default schedule configuration
export const defaultScheduleConfig: ScheduleConfig = {
  type: 'interval',
  intervalValue: 5,
  intervalUnit: 'minutes',
};

export const fieldsConfig = [
  {
    label: 'Workflow',
    name: 'workflowId',
    options: [],
    initialValue: '',
    renderFormField: (config: Record<string, unknown>) => (
      <WorkflowField {...config} />
    ),
  },
  {
    type: 'select',
    label: 'Trigger Type',
    name: 'triggerType',
    options: ['HTTP', 'CRON', 'EMAIL', 'APPLICATION', 'CHANNEL'].map(
      (type) => ({
        label: type,
        value: type,
      })
    ),
    initialValue: 'HTTP',
  },
  {
    label: 'Schedule',
    renderFormField: (config: Record<string, unknown>) => (
      <ScheduleField
        label={config.label as string}
        disabled={config.disabled as boolean}
      />
    ),
  },
  {
    label: 'Static inputs (JSON)',
    name: 'cronInputs',
    initialValue: '',
    colSpan: 'full',
    renderFormField: (config: Record<string, unknown>) => (
      <CronInputsField
        label={config.label as string}
        disabled={config.disabled as boolean}
      />
    ),
  },
  {
    type: 'checkbox',
    label: 'Debug mode',
    name: 'cronDebug',
    initialValue: false,
    hint: 'Capture detailed step events for each scheduled run',
  },
  {
    type: 'checkbox',
    label: 'Debug mode',
    name: 'webhookDebug',
    initialValue: false,
    hint: 'Capture detailed step events for each webhook-triggered run',
  },
  {
    label: 'Webhook verification connection',
    name: 'webhookConnectionId',
    initialValue: '',
    renderFormField: (config: Record<string, unknown>) => (
      <WebhookConnectionField
        label={config.label as string}
        disabled={config.disabled as boolean}
      />
    ),
  },
  {
    type: 'text',
    label: 'Application name',
    name: 'applicationName',
    initialValue: '',
  },
  {
    type: 'text',
    label: 'Event type',
    name: 'eventType',
    initialValue: '',
  },
  {
    label: 'Channel Connection',
    name: 'connectionId',
    initialValue: '',
    renderFormField: (config: Record<string, unknown>) => (
      <ChannelConnectionField
        label={config.label as string}
        disabled={config.disabled as boolean}
      />
    ),
  },
  {
    type: 'select',
    label: 'Session Mode',
    name: 'sessionMode',
    options: [
      {
        label: 'Per sender (each user gets their own conversation)',
        value: 'per_sender',
      },
      {
        label: 'Per trigger (all messages share one session)',
        value: 'per_trigger',
      },
      { label: 'Per message (no session continuity)', value: 'per_message' },
    ],
    initialValue: 'per_sender',
  },
  {
    label: 'Additional Configuration',
    renderFormField: (config: Record<string, unknown>) => (
      <ConfigurationField {...config} />
    ),
  },
  {
    type: 'checkbox',
    label: 'Active',
    name: 'active',
    initialValue: false,
  },
  {
    type: 'checkbox',
    label: 'Single instance',
    name: 'singleInstance',
    initialValue: false,
    hint: 'Only launch a new workflow instance if no other instances of the same workflow are running',
  },
];

// Zod schema for TimeSlot
const timeSlotSchema = z.object({
  hour: z.number().int().min(0).max(23),
  minute: z.number().int().min(0).max(59),
});

// Zod schema for ScheduleConfig
const scheduleConfigSchema = z.object({
  type: z.enum(['interval', 'daily', 'weekly', 'monthly', 'custom']),
  intervalValue: z.number().int().positive().optional(),
  intervalUnit: z.enum(['minutes', 'hours', 'days', 'months']).optional(),
  times: z.array(timeSlotSchema).optional(),
  daysOfWeek: z.array(z.number().int().min(0).max(6)).optional(),
  daysOfMonth: z.array(z.number().int().min(1).max(31)).optional(),
  customExpression: z.string().optional(),
});

// Interval unit validation limits
const intervalLimits: Record<
  string,
  { min: number; max: number; message: string }
> = {
  minutes: {
    min: 1,
    max: 59,
    message: 'Interval must be between 1 and 59 minutes.',
  },
  hours: {
    min: 1,
    max: 23,
    message: 'Interval must be between 1 and 23 hours.',
  },
  days: {
    min: 1,
    max: 31,
    message: 'Interval must be between 1 and 31 days.',
  },
  months: {
    min: 1,
    max: 12,
    message: 'Interval must be between 1 and 12 months.',
  },
};

export const schema = z
  .object({
    workflowId: z.string().nonempty('Please choose a Workflow.'),
    triggerType: z.string().nonempty('Please choose a Trigger Type.'),
    active: z.boolean(),
    singleInstance: z.boolean(),
    scheduleConfig: scheduleConfigSchema.optional(),
    applicationName: z.string().optional(),
    eventType: z.string().optional(),
    connectionId: z.string().optional(),
    sessionMode: z.string().optional(),
    cronInputs: z.string().optional(),
    cronDebug: z.boolean().optional(),
    webhookDebug: z.boolean().optional(),
    webhookConnectionId: z.string().optional(),
    // Loaded configurations can contain non-string values (e.g. CRON
    // `inputs` objects or the `debug` boolean), so values must stay loose.
    configuration: z.record(z.any()).optional().default({}),
    // Legacy fields for backwards compatibility
    time: z.coerce.number().int().nonnegative().optional(),
    timeUnit: z.string().optional(),
  })
  .refine(
    ({ triggerType, scheduleConfig }) => {
      if (triggerType !== 'CRON') {
        return true;
      }

      if (!scheduleConfig) {
        return false;
      }

      switch (scheduleConfig.type) {
        case 'interval': {
          const { intervalValue, intervalUnit } = scheduleConfig;
          if (!intervalValue || !intervalUnit) {
            return false;
          }
          const limits = intervalLimits[intervalUnit];
          if (!limits) {
            return false;
          }
          return intervalValue >= limits.min && intervalValue <= limits.max;
        }

        case 'daily':
        case 'weekly':
        case 'monthly': {
          const { times } = scheduleConfig;
          if (!times || times.length === 0) {
            return false;
          }
          // Validate each time slot
          return times.every(
            (t) =>
              t.hour >= 0 && t.hour <= 23 && t.minute >= 0 && t.minute <= 59
          );
        }

        case 'custom': {
          const { customExpression } = scheduleConfig;
          if (!customExpression) {
            return false;
          }
          // Mirror the server's normalize_cron_expression: 5 fields, or 6
          // fields when the leading seconds field is '0'.
          return isAcceptedCronExpression(customExpression);
        }

        default:
          return false;
      }
    },
    ({ scheduleConfig }) => {
      if (!scheduleConfig) {
        return {
          message: 'Please configure a schedule.',
          path: ['scheduleConfig'],
        };
      }

      if (scheduleConfig.type === 'interval') {
        const unit = scheduleConfig.intervalUnit || 'minutes';
        const limits = intervalLimits[unit];
        return {
          message: limits?.message || 'Invalid interval.',
          path: ['scheduleConfig'],
        };
      }

      if (scheduleConfig.type === 'custom') {
        return {
          message:
            'Please enter a valid cron expression (5 fields: minute hour day-of-month month day-of-week; a leading seconds field is accepted only when it is "0").',
          path: ['scheduleConfig'],
        };
      }

      return {
        message: 'Please configure at least one time.',
        path: ['scheduleConfig'],
      };
    }
  )
  .refine(
    ({ triggerType, connectionId }) => {
      if (triggerType !== 'CHANNEL') return true;
      return !!connectionId;
    },
    {
      message: 'Please select a channel connection.',
      path: ['connectionId'],
    }
  )
  .superRefine(({ triggerType, cronInputs }, ctx) => {
    if (triggerType !== 'CRON') {
      return;
    }
    const error = staticInputsError(cronInputs);
    if (error) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        message: error,
        path: ['cronInputs'],
      });
    }
  });

export const initialValues = fieldsConfig.reduce(
  (initValues: Record<string, unknown>, field) => {
    if (field.name) {
      initValues[field.name] = field.initialValue;
    }
    return initValues;
  },
  {}
);

// Set default schedule config
initialValues.scheduleConfig = defaultScheduleConfig;

// Legacy fields for backwards compatibility
initialValues.time = 5;
initialValues.timeUnit = 'minutes';
