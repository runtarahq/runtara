import { describe, expect, it } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { StructuredErrorDisplay } from './StructuredErrorDisplay';
import { ErrorCategory, ErrorSeverity } from '@/generated/RuntaraRuntimeApi';

describe('StructuredErrorDisplay', () => {
  describe('with structured errors', () => {
    it('renders structured error in compact mode', () => {
      const structuredError = JSON.stringify({
        code: 'OPENAI_RATE_LIMITED',
        message: 'Rate limit exceeded',
        category: ErrorCategory.Transient,
        severity: ErrorSeverity.Error,
        attributes: { status_code: 429 },
      });

      render(<StructuredErrorDisplay error={structuredError} mode="compact" />);

      expect(screen.getByText('OPENAI_RATE_LIMITED')).toBeInTheDocument();
      expect(screen.getByText('transient')).toBeInTheDocument();
      expect(screen.getByText(/OpenAI rate limit reached/)).toBeInTheDocument();
    });

    it('renders structured error in expanded mode', () => {
      const structuredError = JSON.stringify({
        code: 'SHOPIFY_VALIDATION_ERROR',
        message: 'Validation failed',
        category: ErrorCategory.Permanent,
        severity: ErrorSeverity.Error,
        attributes: { errors: ['Invalid field'] },
      });

      render(
        <StructuredErrorDisplay error={structuredError} mode="expanded" />
      );

      expect(screen.getByText('SHOPIFY_VALIDATION_ERROR')).toBeInTheDocument();
      expect(screen.getByText('permanent')).toBeInTheDocument();
      expect(screen.getByText('error')).toBeInTheDocument();
    });

    it('shows attributes section when attributes exist', () => {
      const structuredError = JSON.stringify({
        code: 'OPENAI_UNAUTHORIZED',
        message: 'Auth failed',
        category: ErrorCategory.Permanent,
        severity: ErrorSeverity.Error,
        attributes: { status_code: 401 },
      });

      render(
        <StructuredErrorDisplay error={structuredError} mode="expanded" />
      );

      expect(screen.getByText('Additional Details')).toBeInTheDocument();
    });

    it('expands and collapses attributes section', () => {
      const structuredError = JSON.stringify({
        code: 'TEST_ERROR',
        message: 'Test',
        category: ErrorCategory.Permanent,
        severity: ErrorSeverity.Error,
        attributes: { status_code: 500 },
      });

      render(
        <StructuredErrorDisplay error={structuredError} mode="expanded" />
      );

      const detailsButton = screen.getByText('Additional Details');

      // Initially collapsed - status_code not visible
      expect(screen.queryByText(/status_code/)).not.toBeInTheDocument();

      // Click to expand
      fireEvent.click(detailsButton);

      // Now status_code should be visible
      expect(screen.getByText(/status_code/)).toBeInTheDocument();
      expect(screen.getByText(/500/)).toBeInTheDocument();
    });

    it('shows guidance when available', () => {
      const structuredError = JSON.stringify({
        code: 'OPENAI_RATE_LIMITED',
        message: 'Rate limited',
        category: ErrorCategory.Transient,
        severity: ErrorSeverity.Error,
        attributes: {},
      });

      render(
        <StructuredErrorDisplay
          error={structuredError}
          mode="expanded"
          showGuidance
        />
      );

      expect(screen.getByText(/Suggestion:/)).toBeInTheDocument();
      expect(screen.getByText(/Wait a few minutes/)).toBeInTheDocument();
    });

    it('hides guidance when showGuidance is false', () => {
      const structuredError = JSON.stringify({
        code: 'OPENAI_RATE_LIMITED',
        message: 'Rate limited',
        category: ErrorCategory.Transient,
        severity: ErrorSeverity.Error,
        attributes: {},
      });

      render(
        <StructuredErrorDisplay
          error={structuredError}
          mode="expanded"
          showGuidance={false}
        />
      );

      expect(screen.queryByText(/Suggestion:/)).not.toBeInTheDocument();
    });

    it('uses localized error message', () => {
      const structuredError = JSON.stringify({
        code: 'SHOPIFY_NOT_FOUND',
        message: 'Resource not found',
        category: ErrorCategory.Permanent,
        severity: ErrorSeverity.Error,
        attributes: {},
      });

      render(<StructuredErrorDisplay error={structuredError} mode="compact" />);

      // Should use localized message from error-messages.ts
      expect(
        screen.getByText(/requested Shopify resource was not found/)
      ).toBeInTheDocument();
    });

    it('applies correct color for transient errors', () => {
      const structuredError = JSON.stringify({
        code: 'RATE_LIMITED',
        message: 'Rate limited',
        category: ErrorCategory.Transient,
        severity: ErrorSeverity.Error,
        attributes: {},
      });

      const { container } = render(
        <StructuredErrorDisplay error={structuredError} mode="compact" />
      );

      // Check for warning color class
      const errorDiv = container.querySelector('.bg-warning\\/10');
      expect(errorDiv).toBeInTheDocument();
    });

    it('applies correct color for permanent technical errors', () => {
      const structuredError = JSON.stringify({
        code: 'INVALID_KEY',
        message: 'Invalid key',
        category: ErrorCategory.Permanent,
        severity: ErrorSeverity.Error,
        attributes: {},
      });

      const { container } = render(
        <StructuredErrorDisplay error={structuredError} mode="compact" />
      );

      // Check for destructive color class
      const errorDiv = container.querySelector('.bg-destructive\\/10');
      expect(errorDiv).toBeInTheDocument();
    });

    it('applies correct color for business errors', () => {
      const structuredError = JSON.stringify({
        code: 'CREDIT_LIMIT',
        message: 'Credit limit exceeded',
        category: ErrorCategory.Permanent,
        severity: ErrorSeverity.Warning,
        attributes: {},
      });

      const { container } = render(
        <StructuredErrorDisplay error={structuredError} mode="compact" />
      );

      // Check for warning color class (business errors)
      const errorDiv = container.querySelector('.bg-warning\\/10');
      expect(errorDiv).toBeInTheDocument();
    });
  });

  describe('with legacy plain text errors', () => {
    it('renders plain text error as fallback', () => {
      const plainError = 'This is a plain text error message';

      render(<StructuredErrorDisplay error={plainError} mode="compact" />);

      expect(
        screen.getByText(/This is a plain text error message/)
      ).toBeInTheDocument();
      expect(screen.getByText(/Error:/)).toBeInTheDocument();
    });

    it('renders plain text error with destructive styling', () => {
      const plainError = 'Plain error';

      const { container } = render(
        <StructuredErrorDisplay error={plainError} mode="compact" />
      );

      // Check for destructive color class
      const errorDiv = container.querySelector('.bg-destructive\\/10');
      expect(errorDiv).toBeInTheDocument();
    });
  });

  describe('with null or undefined errors', () => {
    it('renders nothing for null error', () => {
      const { container } = render(
        <StructuredErrorDisplay error={null} mode="compact" />
      );

      expect(container.firstChild).toBeNull();
    });

    it('renders nothing for undefined error', () => {
      const { container } = render(
        <StructuredErrorDisplay error={undefined} mode="compact" />
      );

      expect(container.firstChild).toBeNull();
    });
  });

  describe('prop variations', () => {
    it('hides code badge when showCode is false', () => {
      const structuredError = JSON.stringify({
        code: 'TEST_ERROR',
        message: 'Test',
        category: ErrorCategory.Permanent,
        severity: ErrorSeverity.Error,
        attributes: {},
      });

      render(
        <StructuredErrorDisplay
          error={structuredError}
          mode="compact"
          showCode={false}
        />
      );

      expect(screen.queryByText('TEST_ERROR')).not.toBeInTheDocument();
    });

    it('hides category badge when showCategory is false', () => {
      const structuredError = JSON.stringify({
        code: 'TEST_ERROR',
        message: 'Test',
        category: ErrorCategory.Permanent,
        severity: ErrorSeverity.Error,
        attributes: {},
      });

      render(
        <StructuredErrorDisplay
          error={structuredError}
          mode="compact"
          showCategory={false}
        />
      );

      expect(screen.queryByText('permanent')).not.toBeInTheDocument();
    });

    it('hides attributes section when showAttributes is false', () => {
      const structuredError = JSON.stringify({
        code: 'TEST_ERROR',
        message: 'Test',
        category: ErrorCategory.Permanent,
        severity: ErrorSeverity.Error,
        attributes: { status_code: 500 },
      });

      render(
        <StructuredErrorDisplay
          error={structuredError}
          mode="expanded"
          showAttributes={false}
        />
      );

      expect(screen.queryByText('Additional Details')).not.toBeInTheDocument();
    });

    it('applies custom className', () => {
      const structuredError = JSON.stringify({
        code: 'TEST_ERROR',
        message: 'Test',
        category: ErrorCategory.Permanent,
        severity: ErrorSeverity.Error,
        attributes: {},
      });

      const { container } = render(
        <StructuredErrorDisplay
          error={structuredError}
          mode="compact"
          className="custom-class"
        />
      );

      const errorDiv = container.querySelector('.custom-class');
      expect(errorDiv).toBeInTheDocument();
    });
  });
});
