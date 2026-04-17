import type { Meta, StoryObj } from '@storybook/react';
import { StructuredErrorDisplay } from './StructuredErrorDisplay';

const meta: Meta<typeof StructuredErrorDisplay> = {
  title: 'Feedback/StructuredErrorDisplay',
  component: StructuredErrorDisplay,
  parameters: {
    layout: 'padded',
    docs: {
      description: {
        component:
          'A component for displaying structured errors with color-coded badges, expandable attributes, and actionable guidance. Falls back to plain text for legacy errors.',
      },
    },
  },
  tags: ['autodocs'],
  argTypes: {
    error: {
      control: 'text',
      description: 'Error string (JSON-serialized or plain text)',
    },
    mode: {
      control: 'radio',
      options: ['compact', 'expanded'],
      description: 'Display mode',
    },
    showCode: {
      control: 'boolean',
      description: 'Show error code badge',
    },
    showCategory: {
      control: 'boolean',
      description: 'Show category badge',
    },
    showAttributes: {
      control: 'boolean',
      description: 'Show attributes section',
    },
    showGuidance: {
      control: 'boolean',
      description: 'Show actionable guidance',
    },
  },
};

export default meta;
type Story = StoryObj<typeof StructuredErrorDisplay>;

// Helper to create structured error JSON
const createStructuredError = (error: {
  code: string;
  category: string;
  severity: string;
  message: string;
  attributes?: Record<string, unknown>;
}): string =>
  JSON.stringify({
    code: error.code,
    category: error.category,
    severity: error.severity,
    message: error.message,
    attributes: error.attributes || {},
  });

// Sample errors
const validationError = createStructuredError({
  code: 'VALIDATION_ERROR',
  category: 'validation',
  severity: 'error',
  message: 'Invalid email format provided',
  attributes: {
    field: 'email',
    value: 'invalid-email',
    expected: 'valid email address',
  },
});

const connectionError = createStructuredError({
  code: 'CONNECTION_TIMEOUT',
  category: 'connection',
  severity: 'error',
  message: 'Failed to connect to the remote server',
  attributes: {
    host: 'api.example.com',
    timeout: '30000ms',
    retries: 3,
  },
});

const authError = createStructuredError({
  code: 'AUTH_EXPIRED',
  category: 'authentication',
  severity: 'warning',
  message: 'Your session has expired',
  attributes: {
    expiredAt: '2024-01-15T10:30:00Z',
  },
});

const rateLimitError = createStructuredError({
  code: 'RATE_LIMIT_EXCEEDED',
  category: 'rate_limit',
  severity: 'warning',
  message: 'Too many requests. Please try again later.',
  attributes: {
    limit: 100,
    remaining: 0,
    resetAt: '2024-01-15T11:00:00Z',
  },
});

const businessError = createStructuredError({
  code: 'INSUFFICIENT_BALANCE',
  category: 'business',
  severity: 'error',
  message: 'Insufficient account balance for this transaction',
  attributes: {
    required: 150.0,
    available: 75.5,
    currency: 'USD',
  },
});

const systemError = createStructuredError({
  code: 'INTERNAL_ERROR',
  category: 'system',
  severity: 'critical',
  message: 'An unexpected system error occurred',
  attributes: {
    traceId: 'abc123xyz',
    timestamp: '2024-01-15T10:45:00Z',
  },
});

export const Default: Story = {
  args: {
    error: validationError,
    mode: 'compact',
  },
};

export const Compact: Story = {
  args: {
    error: connectionError,
    mode: 'compact',
  },
};

export const Expanded: Story = {
  args: {
    error: validationError,
    mode: 'expanded',
  },
};

export const PlainTextError: Story = {
  name: 'Plain Text (Legacy)',
  args: {
    error: 'Something went wrong. Please try again.',
    mode: 'compact',
  },
};

export const ValidationError: Story = {
  name: 'Validation Error',
  args: {
    error: validationError,
    mode: 'expanded',
  },
};

export const ConnectionError: Story = {
  name: 'Connection Error',
  args: {
    error: connectionError,
    mode: 'expanded',
  },
};

export const AuthenticationError: Story = {
  name: 'Authentication Error',
  args: {
    error: authError,
    mode: 'expanded',
  },
};

export const RateLimitError: Story = {
  name: 'Rate Limit Error',
  args: {
    error: rateLimitError,
    mode: 'expanded',
  },
};

export const BusinessError: Story = {
  name: 'Business Logic Error',
  args: {
    error: businessError,
    mode: 'expanded',
  },
};

export const SystemError: Story = {
  name: 'System Error',
  args: {
    error: systemError,
    mode: 'expanded',
  },
};

export const WithoutCode: Story = {
  name: 'Without Code Badge',
  args: {
    error: validationError,
    mode: 'compact',
    showCode: false,
  },
};

export const WithoutCategory: Story = {
  name: 'Without Category Badge',
  args: {
    error: validationError,
    mode: 'compact',
    showCategory: false,
  },
};

export const MinimalDisplay: Story = {
  name: 'Minimal (No Badges)',
  args: {
    error: validationError,
    mode: 'compact',
    showCode: false,
    showCategory: false,
  },
};

export const NullError: Story = {
  name: 'No Error (null)',
  args: {
    error: null,
  },
};

export const AllErrorTypes: Story = {
  name: 'All Error Types',
  render: () => (
    <div className="space-y-4">
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          Validation Error
        </p>
        <StructuredErrorDisplay error={validationError} mode="compact" />
      </div>
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          Connection Error
        </p>
        <StructuredErrorDisplay error={connectionError} mode="compact" />
      </div>
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          Authentication Warning
        </p>
        <StructuredErrorDisplay error={authError} mode="compact" />
      </div>
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          Rate Limit Warning
        </p>
        <StructuredErrorDisplay error={rateLimitError} mode="compact" />
      </div>
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          Business Error
        </p>
        <StructuredErrorDisplay error={businessError} mode="compact" />
      </div>
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          System Error (Critical)
        </p>
        <StructuredErrorDisplay error={systemError} mode="compact" />
      </div>
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          Plain Text (Legacy)
        </p>
        <StructuredErrorDisplay
          error="A simple error message without structure"
          mode="compact"
        />
      </div>
    </div>
  ),
};

export const CompactVsExpanded: Story = {
  name: 'Compact vs Expanded',
  render: () => (
    <div className="space-y-6">
      <div>
        <h3 className="text-sm font-medium mb-2">Compact Mode</h3>
        <StructuredErrorDisplay error={validationError} mode="compact" />
      </div>
      <div>
        <h3 className="text-sm font-medium mb-2">Expanded Mode</h3>
        <StructuredErrorDisplay error={validationError} mode="expanded" />
      </div>
    </div>
  ),
};
