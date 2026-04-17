import type { Condition } from '@/shared/components/ui/condition-editor';

/**
 * Pre-built condition templates for handling errors in workflow onError edges.
 * These templates help users quickly build conditions that check the __error context
 * available when a step fails.
 *
 * @see docs/structured-errors.md for __error context structure
 */

interface ErrorConditionTemplate {
  label: string;
  description: string;
  condition: Condition;
}

export const ERROR_CONDITION_TEMPLATES: ErrorConditionTemplate[] = [
  {
    label: 'All transient errors',
    description:
      'Temporary failures that can be retried (rate limits, network issues, etc.)',
    condition: {
      type: 'operation',
      op: 'EQ',
      arguments: [
        { valueType: 'reference', value: '__error.category' },
        { valueType: 'immediate', value: 'transient', immediateType: 'string' },
      ],
    },
  },
  {
    label: 'All permanent errors',
    description:
      'Errors requiring manual intervention (invalid config, missing keys, etc.)',
    condition: {
      type: 'operation',
      op: 'EQ',
      arguments: [
        { valueType: 'reference', value: '__error.category' },
        { valueType: 'immediate', value: 'permanent', immediateType: 'string' },
      ],
    },
  },
  {
    label: 'Rate limit errors',
    description: 'OpenAI, Shopify, or other rate limit errors',
    condition: {
      type: 'operation',
      op: 'CONTAINS',
      arguments: [
        { valueType: 'reference', value: '__error.code' },
        {
          valueType: 'immediate',
          value: 'RATE_LIMITED',
          immediateType: 'string',
        },
      ],
    },
  },
  {
    label: 'Business errors (warnings)',
    description:
      'Permanent errors with warning severity (e.g., credit limit, validation)',
    condition: {
      type: 'operation',
      op: 'AND',
      arguments: [
        {
          type: 'operation',
          op: 'EQ',
          arguments: [
            { valueType: 'reference', value: '__error.category' },
            {
              valueType: 'immediate',
              value: 'permanent',
              immediateType: 'string',
            },
          ],
        },
        {
          type: 'operation',
          op: 'EQ',
          arguments: [
            { valueType: 'reference', value: '__error.severity' },
            {
              valueType: 'immediate',
              value: 'warning',
              immediateType: 'string',
            },
          ],
        },
      ],
    },
  },
  {
    label: 'Critical errors',
    description: 'Errors with critical severity requiring immediate attention',
    condition: {
      type: 'operation',
      op: 'EQ',
      arguments: [
        { valueType: 'reference', value: '__error.severity' },
        { valueType: 'immediate', value: 'critical', immediateType: 'string' },
      ],
    },
  },
  {
    label: 'OpenAI errors',
    description: 'All errors from OpenAI integration',
    condition: {
      type: 'operation',
      op: 'CONTAINS',
      arguments: [
        { valueType: 'reference', value: '__error.code' },
        { valueType: 'immediate', value: 'OPENAI_', immediateType: 'string' },
      ],
    },
  },
  {
    label: 'Shopify errors',
    description: 'All errors from Shopify integration',
    condition: {
      type: 'operation',
      op: 'CONTAINS',
      arguments: [
        { valueType: 'reference', value: '__error.code' },
        { valueType: 'immediate', value: 'SHOPIFY_', immediateType: 'string' },
      ],
    },
  },
  {
    label: 'Authentication errors',
    description: 'Invalid API keys, missing connections, unauthorized access',
    condition: {
      type: 'operation',
      op: 'OR',
      arguments: [
        {
          type: 'operation',
          op: 'CONTAINS',
          arguments: [
            { valueType: 'reference', value: '__error.code' },
            {
              valueType: 'immediate',
              value: 'INVALID_API_KEY',
              immediateType: 'string',
            },
          ],
        },
        {
          type: 'operation',
          op: 'CONTAINS',
          arguments: [
            { valueType: 'reference', value: '__error.code' },
            {
              valueType: 'immediate',
              value: 'UNAUTHORIZED',
              immediateType: 'string',
            },
          ],
        },
        {
          type: 'operation',
          op: 'CONTAINS',
          arguments: [
            { valueType: 'reference', value: '__error.code' },
            {
              valueType: 'immediate',
              value: 'MISSING_CONNECTION',
              immediateType: 'string',
            },
          ],
        },
      ],
    },
  },
];

/**
 * Common error code patterns for quick reference
 * @lintignore Public reference table for constructing error-condition templates.
 */
export const ERROR_CODE_PATTERNS = {
  RATE_LIMITED: '_RATE_LIMITED',
  UNAUTHORIZED: 'UNAUTHORIZED',
  INVALID_KEY: 'INVALID_API_KEY',
  NOT_FOUND: 'NOT_FOUND',
  VALIDATION: 'VALIDATION',
  SERVER_ERROR: 'SERVER_ERROR',
  TIMEOUT: 'TIMEOUT',
} as const;

/**
 * Common error categories
 * @lintignore Public reference table for constructing error-condition templates.
 */
export const ERROR_CATEGORIES = {
  TRANSIENT: 'transient',
  PERMANENT: 'permanent',
} as const;

/**
 * Common error severities
 * @lintignore Public reference table for constructing error-condition templates.
 */
export const ERROR_SEVERITIES = {
  INFO: 'info',
  WARNING: 'warning',
  ERROR: 'error',
  CRITICAL: 'critical',
} as const;
