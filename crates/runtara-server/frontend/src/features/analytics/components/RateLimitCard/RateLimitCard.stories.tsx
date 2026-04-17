import type { Meta, StoryObj } from '@storybook/react';
import { RateLimitCard, RateLimitCardSkeleton } from './index';
import type { RateLimitStatusDto } from '@/generated/RuntaraRuntimeApi';

const meta: Meta<typeof RateLimitCard> = {
  title: 'Analytics/RateLimitCard',
  component: RateLimitCard,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'A card component for displaying rate limit status for API connections. Shows capacity, tokens, configuration, and warnings.',
      },
    },
  },
  tags: ['autodocs'],
};

export default meta;
type Story = StoryObj<typeof RateLimitCard>;

// Sample rate limit data
const healthyRateLimit: RateLimitStatusDto = {
  connectionId: 'conn-001',
  connectionTitle: 'Shopify API',
  integrationId: 'shopify-production',
  config: {
    requestsPerSecond: 2,
    burstSize: 40,
    retryOnLimit: true,
    maxRetries: 3,
    maxWaitMs: 5000,
  },
  state: {
    available: true,
    currentTokens: 35.5,
    learnedLimit: null,
  },
  metrics: {
    capacityPercent: 88.75,
    isRateLimited: false,
    retryAfterMs: null,
  },
};

const warningRateLimit: RateLimitStatusDto = {
  connectionId: 'conn-002',
  connectionTitle: 'WooCommerce API',
  integrationId: 'woo-store-1',
  config: {
    requestsPerSecond: 5,
    burstSize: 100,
    retryOnLimit: true,
    maxRetries: 5,
    maxWaitMs: 10000,
  },
  state: {
    available: true,
    currentTokens: 25.2,
    learnedLimit: null,
  },
  metrics: {
    capacityPercent: 25.2,
    isRateLimited: false,
    retryAfterMs: null,
  },
};

const criticalRateLimit: RateLimitStatusDto = {
  connectionId: 'conn-003',
  connectionTitle: 'Stripe API',
  integrationId: 'stripe-live',
  config: {
    requestsPerSecond: 10,
    burstSize: 50,
    retryOnLimit: true,
    maxRetries: 2,
    maxWaitMs: 3000,
  },
  state: {
    available: true,
    currentTokens: 5.0,
    learnedLimit: null,
  },
  metrics: {
    capacityPercent: 10.0,
    isRateLimited: false,
    retryAfterMs: null,
  },
};

const rateLimitedStatus: RateLimitStatusDto = {
  connectionId: 'conn-004',
  connectionTitle: 'BigCommerce API',
  integrationId: 'bc-main',
  config: {
    requestsPerSecond: 3,
    burstSize: 30,
    retryOnLimit: true,
    maxRetries: 3,
    maxWaitMs: 5000,
  },
  state: {
    available: true,
    currentTokens: 0,
    learnedLimit: null,
  },
  metrics: {
    capacityPercent: 0,
    isRateLimited: true,
    retryAfterMs: 2500,
  },
};

const redisUnavailable: RateLimitStatusDto = {
  connectionId: 'conn-005',
  connectionTitle: 'Magento API',
  integrationId: 'magento-prod',
  config: {
    requestsPerSecond: 2,
    burstSize: 20,
    retryOnLimit: false,
    maxRetries: 0,
    maxWaitMs: 0,
  },
  state: {
    available: false,
    currentTokens: null,
    learnedLimit: null,
  },
  metrics: {
    capacityPercent: null,
    isRateLimited: false,
    retryAfterMs: null,
  },
};

const noConfig: RateLimitStatusDto = {
  connectionId: 'conn-006',
  connectionTitle: 'Custom API',
  integrationId: 'custom-integration',
  config: null,
  state: {
    available: true,
    currentTokens: null,
    learnedLimit: null,
  },
  metrics: {
    capacityPercent: null,
    isRateLimited: false,
    retryAfterMs: null,
  },
};

const learnedLimitDiff: RateLimitStatusDto = {
  connectionId: 'conn-007',
  connectionTitle: 'Amazon SP-API',
  integrationId: 'amazon-seller',
  config: {
    requestsPerSecond: 1,
    burstSize: 10,
    retryOnLimit: true,
    maxRetries: 3,
    maxWaitMs: 5000,
  },
  state: {
    available: true,
    currentTokens: 8.0,
    learnedLimit: 5, // Different from burstSize
  },
  metrics: {
    capacityPercent: 80.0,
    isRateLimited: false,
    retryAfterMs: null,
  },
};

export const Healthy: Story = {
  args: {
    rateLimitStatus: healthyRateLimit,
  },
};

export const Warning: Story = {
  name: 'Warning (Low Capacity)',
  args: {
    rateLimitStatus: warningRateLimit,
  },
};

export const Critical: Story = {
  name: 'Critical (Very Low)',
  args: {
    rateLimitStatus: criticalRateLimit,
  },
};

export const RateLimited: Story = {
  name: 'Rate Limited',
  args: {
    rateLimitStatus: rateLimitedStatus,
  },
};

export const RedisUnavailable: Story = {
  name: 'Redis Unavailable',
  args: {
    rateLimitStatus: redisUnavailable,
  },
};

export const NoConfiguration: Story = {
  name: 'No Configuration',
  args: {
    rateLimitStatus: noConfig,
  },
};

export const LearnedLimitDifference: Story = {
  name: 'Learned Limit Warning',
  args: {
    rateLimitStatus: learnedLimitDiff,
  },
  parameters: {
    docs: {
      description: {
        story:
          'Shows a warning when the API reports a different limit than configured.',
      },
    },
  },
};

export const Skeleton: Story = {
  render: () => <RateLimitCardSkeleton />,
};

export const GridLayout: Story = {
  name: 'Grid Layout',
  render: () => (
    <div className="grid grid-cols-2 gap-4 w-[700px]">
      <RateLimitCard rateLimitStatus={healthyRateLimit} />
      <RateLimitCard rateLimitStatus={warningRateLimit} />
      <RateLimitCard rateLimitStatus={criticalRateLimit} />
      <RateLimitCard rateLimitStatus={rateLimitedStatus} />
    </div>
  ),
};

export const AllStates: Story = {
  name: 'All States Reference',
  render: () => (
    <div className="space-y-4 w-[350px]">
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          Healthy (&gt;50%)
        </p>
        <RateLimitCard rateLimitStatus={healthyRateLimit} />
      </div>
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          Warning (20-50%)
        </p>
        <RateLimitCard rateLimitStatus={warningRateLimit} />
      </div>
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          Critical (&lt;20%)
        </p>
        <RateLimitCard rateLimitStatus={criticalRateLimit} />
      </div>
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          Rate Limited
        </p>
        <RateLimitCard rateLimitStatus={rateLimitedStatus} />
      </div>
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          Loading
        </p>
        <RateLimitCardSkeleton />
      </div>
    </div>
  ),
};
