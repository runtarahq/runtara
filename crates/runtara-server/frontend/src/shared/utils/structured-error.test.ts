import { describe, it, expect } from 'vitest';
import {
  ErrorCategory,
  ErrorSeverity,
} from '@/generated/RuntaraRuntimeApi';
import type { StructuredError } from '@/shared/types/structured-error';
import {
  parseStructuredError,
  isStructuredError,
  getErrorType,
  shouldShowRetryButton,
  getRetryDelay,
  getErrorBadgeVariant,
  getErrorCategoryLabel,
  getErrorSeverityLabel,
} from './structured-error';

describe('parseStructuredError', () => {
  it('should parse valid JSON structured error', () => {
    const jsonError = JSON.stringify({
      code: 'OPENAI_RATE_LIMITED',
      message: 'Rate limit exceeded',
      category: ErrorCategory.Transient,
      severity: ErrorSeverity.Error,
      attributes: { status_code: 429 },
    });

    const result = parseStructuredError(jsonError);

    expect(result).toEqual({
      code: 'OPENAI_RATE_LIMITED',
      message: 'Rate limit exceeded',
      category: ErrorCategory.Transient,
      severity: ErrorSeverity.Error,
      attributes: { status_code: 429 },
    });
  });

  it('should return null for invalid JSON', () => {
    const invalidJson = 'This is not JSON';
    const result = parseStructuredError(invalidJson);
    expect(result).toBeNull();
  });

  it('should return null for JSON missing required fields', () => {
    const incompleteJson = JSON.stringify({
      code: 'ERROR_CODE',
      message: 'Error message',
      // missing category and severity
    });

    const result = parseStructuredError(incompleteJson);
    expect(result).toBeNull();
  });

  it('should return null for null input', () => {
    const result = parseStructuredError(null);
    expect(result).toBeNull();
  });

  it('should return null for undefined input', () => {
    const result = parseStructuredError(undefined);
    expect(result).toBeNull();
  });

  it('should return null for empty string', () => {
    const result = parseStructuredError('');
    expect(result).toBeNull();
  });

  it('should normalize context to attributes for Error step errors (SYN-236)', () => {
    // Backend Error step sends "context" instead of "attributes"
    const errorStepJson = JSON.stringify({
      code: 'TEST_ERROR',
      message: 'Test error message',
      category: ErrorCategory.Permanent,
      severity: ErrorSeverity.Error,
      context: { stepId: 'error-step' },
    });

    const result = parseStructuredError(errorStepJson);

    expect(result).not.toBeNull();
    expect(result!.attributes).toEqual({ stepId: 'error-step' });
  });

  it('should default attributes to empty object when neither attributes nor context is present', () => {
    const minimalJson = JSON.stringify({
      code: 'MINIMAL_ERROR',
      message: 'Minimal error',
      category: ErrorCategory.Permanent,
      severity: ErrorSeverity.Error,
    });

    const result = parseStructuredError(minimalJson);

    expect(result).not.toBeNull();
    expect(result!.attributes).toEqual({});
  });
});

describe('isStructuredError', () => {
  it('should return true for valid structured error', () => {
    const error: StructuredError = {
      code: 'TEST_ERROR',
      message: 'Test message',
      category: ErrorCategory.Permanent,
      severity: ErrorSeverity.Error,
      attributes: {},
    };

    expect(isStructuredError(error)).toBe(true);
  });

  it('should return false for invalid category', () => {
    const error = {
      code: 'TEST_ERROR',
      message: 'Test message',
      category: 'invalid',
      severity: ErrorSeverity.Error,
      attributes: {},
    };

    expect(isStructuredError(error)).toBe(false);
  });

  it('should return false for invalid severity', () => {
    const error = {
      code: 'TEST_ERROR',
      message: 'Test message',
      category: ErrorCategory.Permanent,
      severity: 'invalid',
      attributes: {},
    };

    expect(isStructuredError(error)).toBe(false);
  });

  it('should return false for null', () => {
    expect(isStructuredError(null)).toBe(false);
  });

  it('should return false for non-object', () => {
    expect(isStructuredError('string')).toBe(false);
    expect(isStructuredError(123)).toBe(false);
  });
});

describe('getErrorType', () => {
  it('should return "transient" for transient errors', () => {
    const error: StructuredError = {
      code: 'OPENAI_RATE_LIMITED',
      message: 'Rate limited',
      category: ErrorCategory.Transient,
      severity: ErrorSeverity.Error,
      attributes: {},
    };

    expect(getErrorType(error)).toBe('transient');
  });

  it('should return "business" for permanent warning severity', () => {
    const error: StructuredError = {
      code: 'CREDIT_LIMIT_EXCEEDED',
      message: 'Credit limit exceeded',
      category: ErrorCategory.Permanent,
      severity: ErrorSeverity.Warning,
      attributes: {},
    };

    expect(getErrorType(error)).toBe('business');
  });

  it('should return "technical" for permanent error severity', () => {
    const error: StructuredError = {
      code: 'INVALID_API_KEY',
      message: 'Invalid API key',
      category: ErrorCategory.Permanent,
      severity: ErrorSeverity.Error,
      attributes: {},
    };

    expect(getErrorType(error)).toBe('technical');
  });

  it('should return "technical" for permanent critical severity', () => {
    const error: StructuredError = {
      code: 'SYSTEM_FAILURE',
      message: 'System failure',
      category: ErrorCategory.Permanent,
      severity: ErrorSeverity.Critical,
      attributes: {},
    };

    expect(getErrorType(error)).toBe('technical');
  });
});

describe('shouldShowRetryButton', () => {
  it('should return true for transient errors', () => {
    const jsonError = JSON.stringify({
      code: 'OPENAI_RATE_LIMITED',
      message: 'Rate limited',
      category: ErrorCategory.Transient,
      severity: ErrorSeverity.Error,
      attributes: {},
    });

    expect(shouldShowRetryButton(jsonError)).toBe(true);
  });

  it('should return false for permanent errors', () => {
    const jsonError = JSON.stringify({
      code: 'INVALID_API_KEY',
      message: 'Invalid key',
      category: ErrorCategory.Permanent,
      severity: ErrorSeverity.Error,
      attributes: {},
    });

    expect(shouldShowRetryButton(jsonError)).toBe(false);
  });

  it('should return false for legacy plain string errors', () => {
    expect(shouldShowRetryButton('Plain error message')).toBe(false);
  });

  it('should return false for null', () => {
    expect(shouldShowRetryButton(null)).toBe(false);
  });

  it('should return false for undefined', () => {
    expect(shouldShowRetryButton(undefined)).toBe(false);
  });
});

describe('getRetryDelay', () => {
  it('should return 60000ms for rate limit errors', () => {
    const jsonError = JSON.stringify({
      code: 'OPENAI_RATE_LIMITED',
      message: 'Rate limited',
      category: ErrorCategory.Transient,
      severity: ErrorSeverity.Error,
      attributes: {},
    });

    expect(getRetryDelay(jsonError)).toBe(60000);
  });

  it('should return 60000ms for Shopify rate limit errors', () => {
    const jsonError = JSON.stringify({
      code: 'SHOPIFY_RATE_LIMITED',
      message: 'Rate limited',
      category: ErrorCategory.Transient,
      severity: ErrorSeverity.Error,
      attributes: {},
    });

    expect(getRetryDelay(jsonError)).toBe(60000);
  });

  it('should return 5000ms for other transient errors', () => {
    const jsonError = JSON.stringify({
      code: 'OPENAI_SERVER_ERROR',
      message: 'Server error',
      category: ErrorCategory.Transient,
      severity: ErrorSeverity.Error,
      attributes: {},
    });

    expect(getRetryDelay(jsonError)).toBe(5000);
  });

  it('should return null for permanent errors', () => {
    const jsonError = JSON.stringify({
      code: 'INVALID_API_KEY',
      message: 'Invalid key',
      category: ErrorCategory.Permanent,
      severity: ErrorSeverity.Error,
      attributes: {},
    });

    expect(getRetryDelay(jsonError)).toBeNull();
  });

  it('should return null for legacy errors', () => {
    expect(getRetryDelay('Plain error message')).toBeNull();
  });

  it('should return null for null input', () => {
    expect(getRetryDelay(null)).toBeNull();
  });
});

describe('getErrorBadgeVariant', () => {
  it('should return "secondary" for transient errors', () => {
    const error: StructuredError = {
      code: 'RATE_LIMITED',
      message: 'Rate limited',
      category: ErrorCategory.Transient,
      severity: ErrorSeverity.Error,
      attributes: {},
    };

    expect(getErrorBadgeVariant(error)).toBe('secondary');
  });

  it('should return "outline" for business errors', () => {
    const error: StructuredError = {
      code: 'CREDIT_LIMIT',
      message: 'Credit limit',
      category: ErrorCategory.Permanent,
      severity: ErrorSeverity.Warning,
      attributes: {},
    };

    expect(getErrorBadgeVariant(error)).toBe('outline');
  });

  it('should return "destructive" for technical errors', () => {
    const error: StructuredError = {
      code: 'INVALID_KEY',
      message: 'Invalid key',
      category: ErrorCategory.Permanent,
      severity: ErrorSeverity.Error,
      attributes: {},
    };

    expect(getErrorBadgeVariant(error)).toBe('destructive');
  });
});

describe('getErrorCategoryLabel', () => {
  it('should return correct labels for categories', () => {
    expect(getErrorCategoryLabel(ErrorCategory.Transient)).toBe('Transient');
    expect(getErrorCategoryLabel(ErrorCategory.Permanent)).toBe('Permanent');
  });
});

describe('getErrorSeverityLabel', () => {
  it('should return correct labels for severities', () => {
    expect(getErrorSeverityLabel(ErrorSeverity.Info)).toBe('Info');
    expect(getErrorSeverityLabel(ErrorSeverity.Warning)).toBe('Warning');
    expect(getErrorSeverityLabel(ErrorSeverity.Error)).toBe('Error');
    expect(getErrorSeverityLabel(ErrorSeverity.Critical)).toBe('Critical');
  });
});
