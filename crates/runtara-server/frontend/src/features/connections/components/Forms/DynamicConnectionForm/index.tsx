import { useMemo, useState } from 'react';
import { useForm, useWatch } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { z } from 'zod';
import { toast } from 'sonner';

import type {
  ConnectionTypeDto,
  RateLimitConfigDto,
  RateLimitStatusDto,
} from '@/generated/RuntaraRuntimeApi';
import { NextForm } from '@/shared/components/NextForm';
import { ServiceIcon } from '@/shared/components/service-icon';
import { FormRenderer, type FormAnalysisResult } from '@/shared/forms';

import { ConnectionFormLayout } from '../ConnectionFormLayout';
import { DefaultFileStorageSection } from '../DefaultFileStorageSection';
import { DefaultForSection } from '../DefaultForSection';
import { RateLimitSection } from '../RateLimitSection';
import {
  buildConnectionFormDefinition,
  buildConnectionParameterValues,
  type ConnectionTypeWithForm,
  type EditProjection,
} from './adapter';

const FILE_STORAGE_CATEGORIES = new Set(['file_storage', 'storage']);

type DynamicConnectionFormProps = {
  connectionType: ConnectionTypeDto;
  initValues?: Record<string, unknown>;
  isLoading?: boolean;
  onSubmit: (data: Record<string, unknown>) => void;
  mode?: 'create' | 'edit';
  onDelete?: () => void;
  isDeleting?: boolean;
  rateLimitStatus?: RateLimitStatusDto | null;
  showReconnect?: boolean;
  onReconnect?: () => void;
  isReconnecting?: boolean;
  needsReconnect?: boolean;
};

const frameSchema = z
  .object({
    title: z.string().min(1, 'Please enter a title'),
    rateLimitEnabled: z.boolean(),
    requestsPerSecond: z.coerce.number().min(0).optional(),
    burstSize: z.coerce.number().min(0).optional(),
    maxRetries: z.coerce.number().min(0).optional(),
    maxWaitMs: z.coerce.number().min(0).optional(),
    retryOnLimit: z.boolean(),
    defaultFor: z.array(z.string()).optional(),
    isDefaultFileStorage: z.boolean().optional(),
  })
  .passthrough()
  .superRefine((data, context) => {
    if (!data.rateLimitEnabled) return;
    if (
      !Number.isFinite(data.requestsPerSecond) ||
      Number(data.requestsPerSecond) < 1
    ) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        path: ['requestsPerSecond'],
        message: 'Must be at least 1.',
      });
    }
    if (!Number.isFinite(data.burstSize) || Number(data.burstSize) < 1) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        path: ['burstSize'],
        message: 'Must be at least 1.',
      });
    }
    if (Number(data.burstSize) < Number(data.requestsPerSecond)) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        path: ['burstSize'],
        message: 'Burst size must be ≥ requests per second.',
      });
    }
    if (Number(data.maxRetries) > 100) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        path: ['maxRetries'],
        message: 'Must be 100 or fewer.',
      });
    }
    if (Number(data.maxWaitMs) > 3_600_000) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        path: ['maxWaitMs'],
        message: 'Must be 3600000 ms (1 hour) or less.',
      });
    }
  });

export function DynamicConnectionForm({
  connectionType,
  initValues,
  isLoading,
  onSubmit,
  mode = 'create',
  onDelete,
  isDeleting,
  rateLimitStatus,
  showReconnect,
  onReconnect,
  isReconnecting,
  needsReconnect,
}: DynamicConnectionFormProps) {
  const isFileStorage = FILE_STORAGE_CATEGORIES.has(
    connectionType.category ?? ''
  );
  const definition = useMemo(
    () =>
      buildConnectionFormDefinition(
        connectionType as ConnectionTypeWithForm,
        mode
      ),
    [connectionType, mode]
  );
  const canonicalDefaults = useMemo(
    () => buildConnectionParameterValues(definition, initValues, mode),
    [definition, initValues, mode]
  );
  const defaultRateLimit = connectionType.defaultRateLimitConfig;
  const existingRateLimit = initValues?.rateLimitConfig as
    | RateLimitConfigDto
    | undefined;
  const form = useForm<Record<string, unknown>>({
    resolver: zodResolver(frameSchema),
    values: {
      ...canonicalDefaults,
      rateLimitEnabled: Boolean(existingRateLimit),
      requestsPerSecond:
        existingRateLimit?.requestsPerSecond ??
        defaultRateLimit?.requestsPerSecond ??
        '',
      burstSize:
        existingRateLimit?.burstSize ?? defaultRateLimit?.burstSize ?? '',
      maxRetries:
        existingRateLimit?.maxRetries ?? defaultRateLimit?.maxRetries ?? '',
      maxWaitMs:
        existingRateLimit?.maxWaitMs ?? defaultRateLimit?.maxWaitMs ?? '',
      retryOnLimit:
        existingRateLimit?.retryOnLimit ??
        defaultRateLimit?.retryOnLimit ??
        true,
      defaultFor: Array.isArray(initValues?.defaultFor)
        ? initValues.defaultFor
        : [],
      isDefaultFileStorage: Boolean(initValues?.isDefaultFileStorage),
    },
  });
  const watched = useWatch({ control: form.control });
  const [analysis, setAnalysis] = useState<FormAnalysisResult | null>(null);
  const [submitAttempt, setSubmitAttempt] = useState(0);
  const editProjection = initValues?.editProjection as
    | EditProjection
    | undefined;
  const formValue = Object.fromEntries(
    Object.keys(definition.fields).map((name) => [name, watched[name]])
  );

  const handleSubmit = (values: Record<string, unknown>) => {
    setSubmitAttempt((attempt) => attempt + 1);
    if (!analysis?.wasmAvailable || !analysis.valid) {
      toast.error('Fix the highlighted connection fields before saving.');
      return;
    }
    const parameters = Object.fromEntries(
      Object.keys(
        (connectionType as ConnectionTypeWithForm).formDefinition?.fields ?? {}
      ).map((name) => [name, values[name]])
    );
    onSubmit({
      ...values,
      ...parameters,
    });
  };

  return (
    <NextForm
      form={form}
      onSubmit={handleSubmit}
      className="w-full"
      renderActions={() => null}
      renderContent={() => (
        <ConnectionFormLayout
          title={mode === 'edit' ? 'Edit connection' : 'Create connection'}
          isLoading={isLoading}
          isSubmitDisabled={analysis?.wasmAvailable === false}
          submitLabel={mode === 'edit' ? 'Save changes' : 'Create connection'}
          loadingLabel={mode === 'edit' ? 'Saving...' : 'Creating...'}
          editNotice={
            mode === 'edit'
              ? 'Stored secrets stay hidden. Enter new values to update them.'
              : undefined
          }
          integrationIcon={
            <ServiceIcon
              serviceId={connectionType.integrationId || undefined}
              category={connectionType.category || undefined}
            />
          }
          integrationName={connectionType.displayName || undefined}
          integrationCategory={connectionType.category || undefined}
          onDelete={onDelete}
          isDeleting={isDeleting}
          showReconnect={showReconnect}
          onReconnect={onReconnect}
          isReconnecting={isReconnecting}
          needsReconnect={needsReconnect}
        >
          <div className="space-y-6">
            <FormRenderer
              definition={definition}
              value={formValue}
              onChange={(next) => {
                for (const [name, value] of Object.entries(next)) {
                  form.setValue(name, value, { shouldDirty: true });
                }
              }}
              onAnalysisChange={setAnalysis}
              submitAttempt={submitAttempt}
              fieldAnnotations={Object.fromEntries(
                Object.entries(editProjection?.secretState ?? {}).map(
                  ([name, state]) => [
                    name,
                    <p
                      key={name}
                      className="text-xs text-emerald-700 dark:text-emerald-400"
                    >
                      {state.configured
                        ? 'A secret is configured. Enter a new value only to replace it.'
                        : 'No secret is configured.'}
                    </p>,
                  ]
                )
              )}
            />
            {isFileStorage && <DefaultFileStorageSection />}
            <DefaultForSection connectionType={connectionType} />
            <RateLimitSection
              defaultConfig={connectionType.defaultRateLimitConfig}
              liveStatus={rateLimitStatus}
            />
          </div>
        </ConnectionFormLayout>
      )}
    />
  );
}
