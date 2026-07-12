import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { useForm, useWatch } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { z } from 'zod';
import { toast } from 'sonner';
import { Loader2, Trash2 } from 'lucide-react';

import type {
  ConnectionStatus,
  ConnectionTypeDto,
  RateLimitConfigDto,
  RateLimitStatusDto,
} from '@/generated/RuntaraRuntimeApi';
import { NextForm } from '@/shared/components/NextForm';
import { ServiceIcon } from '@/shared/components/service-icon';
import { Button } from '@/shared/components/ui/button';
import { useNavigationBlockerStore } from '@/shared/stores/navigationBlockerStore';
import {
  analyzeFormWithRust,
  FormRenderer,
  type FormAnalysisResult,
} from '@/shared/forms';

import { ConnectionPageShell } from '../ConnectionPageShell';
import { ConnectionSaveBar } from '../ConnectionSaveBar';
import { CollapsedSection } from '../CollapsedSection';
import { ConnectionStatusCard } from '../../ConnectionStatusCard';
import { patchStripsAuthorization } from '../../../utils/reauthorization';
import { DefaultFileStorageSection } from '../DefaultFileStorageSection';
import { DefaultForSection } from '../DefaultForSection';
import { RateLimitSection } from '../RateLimitSection';
import {
  buildConnectionFormDefinition,
  buildConnectionCreateParameters,
  buildConnectionParameterValues,
  type ConnectionTypeWithForm,
  type EditProjection,
} from './adapter';
import { ConnectionFieldFrame } from './ConnectionFieldFrame';

const FILE_STORAGE_CATEGORIES = new Set(['file_storage', 'storage']);
const EMPTY_SECRET_STATE: NonNullable<EditProjection['secretState']> = {};

type DynamicConnectionFormProps = {
  connectionType: ConnectionTypeDto;
  initValues?: Record<string, unknown>;
  isLoading?: boolean;
  onSubmit: (
    data: Record<string, unknown>,
    operations: ConnectionFormOperations
  ) => void;
  mode?: 'create' | 'edit';
  onDelete?: () => void;
  isDeleting?: boolean;
  rateLimitStatus?: RateLimitStatusDto | null;
  showReconnect?: boolean;
  onReconnect?: () => void;
  isReconnecting?: boolean;
  needsReconnect?: boolean;
  conflictNotice?: ReactNode;
};

export interface ConnectionFormOperations {
  clearSecrets: string[];
  dirtyFields: string[];
}

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
  conflictNotice,
}: DynamicConnectionFormProps) {
  const isFileStorage = FILE_STORAGE_CATEGORIES.has(
    connectionType.category ?? ''
  );
  const editProjection = initValues?.editProjection as
    | EditProjection
    | undefined;
  const secretState = editProjection?.secretState ?? EMPTY_SECRET_STATE;
  const [clearedSecrets, setClearedSecrets] = useState<Set<string>>(
    () => new Set()
  );
  const baseDefinition = useMemo(
    () =>
      buildConnectionFormDefinition(
        connectionType as ConnectionTypeWithForm,
        mode,
        secretState
      ),
    [connectionType, mode, secretState]
  );
  const definition = useMemo(
    () =>
      buildConnectionFormDefinition(
        connectionType as ConnectionTypeWithForm,
        mode,
        secretState,
        clearedSecrets
      ),
    [clearedSecrets, connectionType, mode, secretState]
  );
  const canonicalDefaults = useMemo(
    () => buildConnectionParameterValues(baseDefinition, initValues, mode),
    [baseDefinition, initValues, mode]
  );
  const defaultRateLimit = connectionType.defaultRateLimitConfig;
  const existingRateLimit = initValues?.rateLimitConfig as
    | RateLimitConfigDto
    | undefined;
  const formValues = useMemo(
    () => ({
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
    }),
    [canonicalDefaults, defaultRateLimit, existingRateLimit, initValues]
  );
  const form = useForm<Record<string, unknown>>({
    resolver: zodResolver(frameSchema),
    values: formValues,
    // Background refetches must not clobber in-progress edits; the explicit
    // reset on version change below handles the post-save clean state.
    resetOptions: { keepDirtyValues: true },
  });
  const watched = useWatch({ control: form.control });
  const [analysis, setAnalysis] = useState<FormAnalysisResult | null>(null);
  const [submitAttempt, setSubmitAttempt] = useState(0);
  const formValue = Object.fromEntries(
    Object.keys(definition.fields).map((name) => [name, watched[name]])
  );
  const { isDirty, dirtyFields } = form.formState;
  const hasPendingChanges = isDirty || clearedSecrets.size > 0;
  const dirtyCount = Object.values(dirtyFields).filter(Boolean).length;
  const setBlocker = useNavigationBlockerStore((state) => state.setBlocker);

  // A version bump means the server accepted a write (ours or someone
  // else's): re-sync to the fresh projection and drop staged edits so
  // secret inputs empty out and the save bar collapses.
  useEffect(() => {
    setClearedSecrets(new Set());
    form.reset(formValues);
    // formValues is intentionally not a dependency: only a version change
    // may discard in-progress edits.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editProjection?.version]);

  useEffect(() => {
    setBlocker(hasPendingChanges, () => {
      form.reset();
      setClearedSecrets(new Set());
    });
    return () => setBlocker(false);
  }, [hasPendingChanges, setBlocker, form]);

  useEffect(() => {
    if (!hasPendingChanges) return;
    const handler = (event: BeforeUnloadEvent) => {
      event.preventDefault();
    };
    window.addEventListener('beforeunload', handler);
    return () => window.removeEventListener('beforeunload', handler);
  }, [hasPendingChanges]);

  const handleDiscard = () => {
    // The controlled FieldControls read from useWatch, and with
    // keepDirtyValues a bare reset() doesn't always re-emit; set each field
    // back explicitly (the same path typing uses) then clear dirty state.
    for (const [name, value] of Object.entries(formValues)) {
      form.setValue(name, value, { shouldDirty: false });
    }
    form.reset(formValues);
    setClearedSecrets(new Set());
  };

  const handleSubmit = async (values: Record<string, unknown>) => {
    const parameters = buildConnectionCreateParameters(
      (connectionType as ConnectionTypeWithForm).formDefinition ?? {
        fields: {},
      },
      values
    );
    const submissionData = Object.fromEntries(
      Object.keys(definition.fields).map((name) => [name, values[name]])
    );
    const submissionAnalysis = await analyzeFormWithRust(
      definition,
      submissionData
    );
    setAnalysis(submissionAnalysis);
    setSubmitAttempt((attempt) => attempt + 1);
    if (!submissionAnalysis.wasmAvailable || !submissionAnalysis.valid) {
      toast.error('Fix the highlighted connection fields before saving.');
      return;
    }
    onSubmit(
      {
        ...values,
        ...parameters,
      },
      {
        clearSecrets: [...clearedSecrets].sort(),
        dirtyFields: Object.entries(form.formState.dirtyFields)
          .filter(([, dirty]) => dirty === true)
          .map(([name]) => name)
          .sort(),
      }
    );
  };

  // Interim header action: Delete moves into the danger zone (PR-3).
  const headerActions =
    mode === 'edit' && onDelete ? (
      <Button
        type="button"
        variant="ghost"
        size="sm"
        onClick={onDelete}
        disabled={isDeleting}
        className="text-red-600 hover:text-red-700 hover:bg-red-50 dark:hover:bg-red-900/30"
      >
        {isDeleting ? (
          <Loader2 className="w-4 h-4 mr-1.5 animate-spin" />
        ) : (
          <Trash2 className="w-4 h-4 mr-1.5" />
        )}
        Delete
      </Button>
    ) : undefined;

  // Descriptor parameter fields (not the title/rate-limit frame) with unsaved
  // edits, plus staged secret clears — the set the status card guards against
  // a stale-credential reconnect.
  const touchedParamFields = new Set<string>([
    ...Object.entries(dirtyFields)
      .filter(([name, dirty]) => dirty === true && name in definition.fields)
      .map(([name]) => name),
    ...clearedSecrets,
  ]);
  const hasParamChanges = touchedParamFields.size > 0;
  const hasReauthChanges = patchStripsAuthorization(
    connectionType.fieldBehaviors,
    touchedParamFields
  );
  const configuredSecretCount = Object.values(secretState).filter(
    (state) => state.configured
  ).length;

  const showSaveBar =
    mode === 'create' || hasPendingChanges || Boolean(isLoading);

  return (
    <NextForm
      form={form}
      onSubmit={handleSubmit}
      className="w-full"
      renderActions={() => null}
      renderContent={() => (
        <ConnectionPageShell
          mode={mode}
          integrationIcon={
            <ServiceIcon
              serviceId={connectionType.integrationId || undefined}
              category={connectionType.category || undefined}
            />
          }
          integrationName={connectionType.displayName || undefined}
          integrationCategory={connectionType.category || undefined}
          headerActions={headerActions}
          footer={
            showSaveBar ? (
              <ConnectionSaveBar
                isLoading={isLoading}
                isSubmitDisabled={analysis?.wasmAvailable === false}
                submitLabel={
                  mode === 'edit' ? 'Save changes' : 'Create connection'
                }
                loadingLabel={mode === 'edit' ? 'Saving...' : 'Creating...'}
                dirtyCount={dirtyCount}
                clearedCount={clearedSecrets.size}
                showDiscard={mode === 'edit' && hasPendingChanges}
                onDiscard={handleDiscard}
              />
            ) : undefined
          }
        >
          <div className="space-y-6">
            {conflictNotice}
            {mode === 'edit' && (
              <ConnectionStatusCard
                status={
                  ((initValues as { status?: ConnectionStatus })?.status ??
                    'UNKNOWN') as ConnectionStatus
                }
                connectionType={connectionType as ConnectionTypeWithForm}
                values={editProjection?.values as Record<string, unknown>}
                configuredSecretCount={configuredSecretCount}
                updatedAt={(initValues as { updatedAt?: string })?.updatedAt}
                isOAuth={Boolean(showReconnect)}
                onReconnect={onReconnect}
                onSaveAndReconnect={() => form.handleSubmit(handleSubmit)()}
                isReconnecting={isReconnecting}
                hasParamChanges={hasParamChanges}
                hasReauthChanges={hasReauthChanges}
              />
            )}
            <FormRenderer
              definition={definition}
              value={formValue}
              onChange={(next) => {
                for (const [name, value] of Object.entries(next)) {
                  form.setValue(name, value, { shouldDirty: true });
                }
              }}
              frame={{
                commitField: ({ fieldName, value }) => {
                  form.setValue(fieldName, value, { shouldDirty: true });
                  if (value !== '' && value !== null && value !== undefined) {
                    setClearedSecrets((current) => {
                      if (!current.has(fieldName)) return current;
                      const next = new Set(current);
                      next.delete(fieldName);
                      return next;
                    });
                  }
                },
              }}
              onAnalysisChange={setAnalysis}
              submitAttempt={submitAttempt}
              fieldAnnotations={Object.fromEntries(
                Object.entries(secretState).map(([name, state]) => {
                  const field = definition.fields[name];
                  const behavior = connectionType.fieldBehaviors[name];
                  const label = field?.label ?? name.replace(/_/g, ' ');
                  return [
                    name,
                    <ConnectionFieldFrame
                      key={name}
                      label={label}
                      configured={state.configured}
                      clearable={state.clearable}
                      cleared={clearedSecrets.has(name)}
                      requiresReauthorization={
                        behavior?.requiresReauthorization
                      }
                      onClear={() => {
                        setClearedSecrets((current) =>
                          new Set(current).add(name)
                        );
                        form.setValue(name, '', { shouldDirty: true });
                      }}
                      onUndoClear={() => {
                        setClearedSecrets((current) => {
                          const next = new Set(current);
                          next.delete(name);
                          return next;
                        });
                      }}
                    />,
                  ];
                })
              )}
            />
            {isFileStorage && <DefaultFileStorageSection />}
            <DefaultForSection connectionType={connectionType} />
            <CollapsedSection
              label="Advanced"
              description="Rate limiting and retry behavior"
              forceOpen={Boolean(watched.rateLimitEnabled)}
            >
              <RateLimitSection
                defaultConfig={connectionType.defaultRateLimitConfig}
                liveStatus={rateLimitStatus}
              />
            </CollapsedSection>
          </div>
        </ConnectionPageShell>
      )}
    />
  );
}
