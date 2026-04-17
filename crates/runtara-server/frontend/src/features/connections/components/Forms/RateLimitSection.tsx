import { useWatch } from 'react-hook-form';
import { Gauge } from 'lucide-react';
import { FormSection } from './FormSection';
import { TextInput } from '@/shared/components/text-input';
import { CheckboxInput } from '@/shared/components/checkbox-input';
import { RateLimitConfigDto } from '@/generated/RuntaraRuntimeApi';

type RateLimitSectionProps = {
  defaultConfig?: RateLimitConfigDto | null;
};

export function RateLimitSection({ defaultConfig }: RateLimitSectionProps) {
  const rateLimitEnabled = useWatch({ name: 'rateLimitEnabled' });

  return (
    <FormSection title="Rate Limiting" icon={Gauge} optional>
      <div className="space-y-4">
        <CheckboxInput
          name="rateLimitEnabled"
          label="Override default rate limits"
        />

        {!rateLimitEnabled && defaultConfig && (
          <div className="text-xs text-slate-500 bg-slate-50 rounded-md px-3 py-2 dark:bg-slate-800/50 dark:text-slate-400">
            Using defaults: {defaultConfig.requestsPerSecond} req/s, burst{' '}
            {defaultConfig.burstSize},{' '}
            {defaultConfig.retryOnLimit
              ? `auto-retry enabled (max ${defaultConfig.maxRetries} retries, ${defaultConfig.maxWaitMs}ms wait)`
              : 'no auto-retry'}
          </div>
        )}

        {!rateLimitEnabled && !defaultConfig && (
          <div className="text-xs text-slate-500 bg-slate-50 rounded-md px-3 py-2 dark:bg-slate-800/50 dark:text-slate-400">
            No default rate limits configured for this connection type.
          </div>
        )}

        {rateLimitEnabled && (
          <div className="grid grid-cols-2 gap-4">
            <TextInput
              name="requestsPerSecond"
              label="Requests per second"
              type="number"
              placeholder={defaultConfig?.requestsPerSecond?.toString()}
              description="Token refill rate"
            />
            <TextInput
              name="burstSize"
              label="Burst size"
              type="number"
              placeholder={defaultConfig?.burstSize?.toString()}
              description="Maximum token capacity"
            />
            <TextInput
              name="maxRetries"
              label="Max retries"
              type="number"
              placeholder={defaultConfig?.maxRetries?.toString()}
              description="Maximum retry attempts"
            />
            <TextInput
              name="maxWaitMs"
              label="Max wait (ms)"
              type="number"
              placeholder={defaultConfig?.maxWaitMs?.toString()}
              description="Maximum cumulative wait time"
            />
            <div className="col-span-2">
              <CheckboxInput
                name="retryOnLimit"
                label="Auto-retry on rate limit"
              />
            </div>
          </div>
        )}
      </div>
    </FormSection>
  );
}
