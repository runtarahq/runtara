/* eslint-disable react-refresh/only-export-components */
// This file exports both components and configuration because the field configs
// contain JSX (renderFormField, renderComponent) which tightly couples them to components.
// Separating would require complex refactoring with circular dependency resolution.
import { z } from 'zod';
import {
  useState,
  createContext,
  useContext,
  useCallback,
  useEffect,
} from 'react';
import { useFormContext, useWatch } from 'react-hook-form';
import { InputMappingField } from './InputMappingField';
import { TestAgentInline } from './TestAgentButton/TestAgentInline';
import { EmbedWorkflowConfigField } from './EmbedWorkflowConfigField';
import { NameField } from './NameField';
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from '@/shared/components/ui/tabs';
import { FinishStepField } from './FinishStepField';
import { SplitStepField } from './SplitStepField';
import { ErrorStepField } from './ErrorStepField';
import { FilterStepField } from './FilterStepField';
import { GroupByStepField } from './GroupByStepField';
import { AiAgentStepField } from './AiAgentStepField';
import { WaitForSignalStepField } from './WaitForSignalStepField';
import { LogStepField } from './LogStepField';
import { WhileStepField } from './WhileStepField';
import { DelayStepField } from './DelayStepField';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import { Switch as ToggleSwitch } from '@/shared/components/ui/switch';
import { Textarea } from '@/shared/components/ui/textarea';

// Wrapper component for EmbedWorkflowConfigField that uses react-hook-form
function EmbedWorkflowFieldRenderer() {
  const form = useFormContext();
  const stepType = useWatch({ name: 'stepType', control: form.control });
  const childWorkflowId = useWatch({
    name: 'childWorkflowId',
    control: form.control,
  });
  const childVersion = useWatch({
    name: 'childVersion',
    control: form.control,
  });

  // Memoize callbacks to prevent infinite re-renders
  const handleWorkflowIdChange = useCallback(
    (value: string) => {
      form.setValue('childWorkflowId', value, {
        shouldValidate: true,
        shouldDirty: true,
        shouldTouch: true,
      });
      // Clear stale input mappings from the previously selected child workflow
      form.setValue('inputMapping', [], {
        shouldDirty: true,
      });
    },
    [form]
  );

  const handleVersionChange = useCallback(
    (value: string) => {
      form.setValue('childVersion', value, {
        shouldValidate: true,
        shouldDirty: true,
        shouldTouch: true,
      });
    },
    [form]
  );

  // Only show for EmbedWorkflow steps
  if (!stepType || stepType !== 'EmbedWorkflow') {
    return null;
  }

  return (
    <EmbedWorkflowConfigField
      workflowIdValue={childWorkflowId || ''}
      versionValue={childVersion || ''}
      onWorkflowIdChange={handleWorkflowIdChange}
      onVersionChange={handleVersionChange}
      disabled={false}
    />
  );
}

// Context to track active tab
const TabContext = createContext<{
  activeTab: string;
  setActiveTab: (tab: string) => void;
}>({
  activeTab: 'main',
  setActiveTab: () => {},
});

export const useTabContext = () => useContext(TabContext);

function FormTabs() {
  const { activeTab, setActiveTab } = useTabContext();
  const form = useFormContext();
  const capabilityId = useWatch({
    name: 'capabilityId',
    control: form.control,
  });

  // Don't show tabs until capability is selected
  if (!capabilityId) {
    return null;
  }

  return (
    <div className="-my-3">
      <Tabs value={activeTab} onValueChange={setActiveTab} className="w-full">
        <TabsList className="grid w-full grid-cols-2">
          <TabsTrigger value="main">Main</TabsTrigger>
          <TabsTrigger value="testing">Testing</TabsTrigger>
        </TabsList>
        <TabsContent value="main" className="space-y-4 mt-4">
          {/* Main tab content is now rendered below the tabs via mainTabFieldsConfig */}
        </TabsContent>
        <TabsContent value="testing" className="space-y-4 mt-4">
          <TestAgentInline />
        </TabsContent>
      </Tabs>
    </div>
  );
}

const DURABLE_STEP_TYPES = new Set([
  'Agent',
  'Split',
  'EmbedWorkflow',
  'Delay',
  'AiAgent',
]);

const RETRY_STEP_TYPES = new Set(['Agent', 'EmbedWorkflow']);

function StepAdvancedFields() {
  const { activeTab } = useTabContext();
  const form = useFormContext();
  const stepType = useWatch({ name: 'stepType', control: form.control });
  const breakpoint = useWatch({ name: 'breakpoint', control: form.control });
  const durable = useWatch({ name: 'durable', control: form.control });
  const maxRetries = useWatch({ name: 'maxRetries', control: form.control });
  const retryDelay = useWatch({ name: 'retryDelay', control: form.control });
  const timeout = useWatch({ name: 'timeout', control: form.control });
  const compensation = useWatch({
    name: 'compensation',
    control: form.control,
  });
  const [compensationDraft, setCompensationDraft] = useState('');
  const [compensationError, setCompensationError] = useState<string | null>(
    null
  );

  useEffect(() => {
    if (stepType !== 'Agent') return;
    setCompensationDraft(
      compensation ? JSON.stringify(compensation, null, 2) : ''
    );
    setCompensationError(null);
  }, [stepType, compensation]);

  if (activeTab !== 'main' || !stepType || stepType === 'Start') {
    return null;
  }

  const showDurable = DURABLE_STEP_TYPES.has(stepType);
  const showRetries = RETRY_STEP_TYPES.has(stepType);
  const showCompensation = stepType === 'Agent';

  return (
    <div className="space-y-4 rounded-md border p-3">
      <div className="flex items-center justify-between gap-4">
        <div className="space-y-0.5">
          <Label className="text-sm">Breakpoint</Label>
          <p className="text-xs text-muted-foreground">
            Pause before this step when debugging.
          </p>
        </div>
        <ToggleSwitch
          checked={breakpoint === true}
          onCheckedChange={(checked) =>
            form.setValue('breakpoint', checked, { shouldDirty: true })
          }
        />
      </div>

      {showDurable && (
        <div className="flex items-center justify-between gap-4">
          <div className="space-y-0.5">
            <Label className="text-sm">Durable</Label>
            <p className="text-xs text-muted-foreground">
              Keep this step suspendable and resumable.
            </p>
          </div>
          <ToggleSwitch
            checked={durable !== false}
            onCheckedChange={(checked) =>
              form.setValue('durable', checked, { shouldDirty: true })
            }
          />
        </div>
      )}

      {showRetries && (
        <>
          <div className="grid grid-cols-3 gap-3">
            <div className="space-y-1">
              <Label className="text-sm">Retries</Label>
              <Input
                type="number"
                min={0}
                value={maxRetries ?? ''}
                onChange={(event) =>
                  form.setValue(
                    'maxRetries',
                    event.target.value === ''
                      ? undefined
                      : Number(event.target.value),
                    { shouldDirty: true }
                  )
                }
              />
            </div>
            <div className="space-y-1">
              <Label className="text-sm">Retry delay (ms)</Label>
              <Input
                type="number"
                min={0}
                value={retryDelay ?? ''}
                onChange={(event) =>
                  form.setValue(
                    'retryDelay',
                    event.target.value === ''
                      ? undefined
                      : Number(event.target.value),
                    { shouldDirty: true }
                  )
                }
              />
            </div>
            <div className="space-y-1">
              <Label className="text-sm">Timeout (ms)</Label>
              <Input
                type="number"
                min={0}
                value={timeout ?? ''}
                onChange={(event) =>
                  form.setValue(
                    'timeout',
                    event.target.value === ''
                      ? undefined
                      : Number(event.target.value),
                    { shouldDirty: true }
                  )
                }
              />
            </div>
          </div>
          <p className="text-xs text-muted-foreground">
            Timeout is accepted by the DSL for these steps; runtime validation
            currently reports it as warning-only.
          </p>
        </>
      )}

      {showCompensation && (
        <div className="space-y-2">
          <div className="space-y-0.5">
            <Label className="text-sm">Compensation JSON</Label>
            <p className="text-xs text-muted-foreground">
              Accepted by the DSL; runtime validation reports compensation as
              warning-only.
            </p>
          </div>
          <Textarea
            value={compensationDraft}
            onChange={(event) => {
              const nextDraft = event.target.value;
              setCompensationDraft(nextDraft);

              if (!nextDraft.trim()) {
                form.setValue('compensation', undefined, {
                  shouldDirty: true,
                });
                setCompensationError(null);
                return;
              }

              try {
                const parsed = JSON.parse(nextDraft);
                if (
                  !parsed ||
                  typeof parsed !== 'object' ||
                  Array.isArray(parsed)
                ) {
                  setCompensationError('Compensation must be a JSON object.');
                  return;
                }

                form.setValue('compensation', parsed, { shouldDirty: true });
                setCompensationError(null);
              } catch (error) {
                setCompensationError(
                  error instanceof Error ? error.message : 'Invalid JSON.'
                );
              }
            }}
            className="min-h-[120px] font-mono text-xs"
            spellCheck={false}
            placeholder='{"compensationStep":"rollback"}'
          />
          {compensationError && (
            <p className="text-xs text-destructive">{compensationError}</p>
          )}
        </div>
      )}
    </div>
  );
}

// Basic configuration fields (above tabs)
const basicFieldsConfig = [
  {
    type: 'hidden',
    label: '',
    name: 'stepType',
    initialValue: 'Agent',
    colSpan: 'full',
  },
  {
    type: 'text',
    label: 'Name',
    name: 'name',
    initialValue: 'New operation',
    description: 'A descriptive name for this workflow step',
    colSpan: 'full',
    renderFormField: (config: Record<string, unknown>) => (
      <NameField name={config.name as string} />
    ),
  },
  {
    label: '',
    name: 'agentId',
    initialValue: '',
    colSpan: 'full',
    type: 'hidden',
  },
  {
    label: '',
    name: 'capabilityId',
    initialValue: '',
    colSpan: 'full',
    type: 'hidden',
  },

  {
    label: '',
    name: 'childWorkflowId',
    initialValue: '',
    colSpan: 'full',
    type: 'hidden', // Field value, actual UI is rendered by embedWorkflowConfig
  },
  {
    label: '',
    name: 'childVersion',
    initialValue: 'latest',
    colSpan: 'full',
    type: 'hidden', // Field value, actual UI is rendered by embedWorkflowConfig
  },
  {
    label: '',
    name: 'embedWorkflowConfig',
    initialValue: undefined,
    colSpan: 'full',
    renderFormField: () => <EmbedWorkflowFieldRenderer />,
  },
  {
    label: '',
    name: 'connectionId',
    initialValue: '',
    colSpan: 'full',
    type: 'hidden',
  },
  {
    label: '',
    name: 'inputSchema',
    initialValue: undefined,
    colSpan: 'full',
    type: 'hidden',
  },
  {
    label: '',
    name: 'inputSchemaFields',
    initialValue: [],
    colSpan: 'full',
    type: 'hidden',
  },
  {
    label: '',
    name: 'startMode',
    initialValue: undefined,
    colSpan: 'full',
    type: 'hidden',
  },
  {
    label: '',
    name: 'selectedTriggerId',
    initialValue: '',
    colSpan: 'full',
    type: 'hidden',
  },
  // Split step schema fields
  {
    label: '',
    name: 'outputSchema',
    initialValue: undefined,
    colSpan: 'full',
    type: 'hidden',
  },
  {
    label: '',
    name: 'splitInputSchemaFields',
    initialValue: [],
    colSpan: 'full',
    type: 'hidden',
  },
  {
    label: '',
    name: 'splitOutputSchemaFields',
    initialValue: [],
    colSpan: 'full',
    type: 'hidden',
  },
  // Split step config fields
  {
    label: '',
    name: 'splitVariablesFields',
    initialValue: [],
    colSpan: 'full',
    type: 'hidden',
  },
  {
    label: '',
    name: 'splitParallelism',
    initialValue: 0,
    colSpan: 'full',
    type: 'hidden',
  },
  {
    label: '',
    name: 'splitSequential',
    initialValue: false,
    colSpan: 'full',
    type: 'hidden',
  },
  {
    label: '',
    name: 'splitDontStopOnFailed',
    initialValue: false,
    colSpan: 'full',
    type: 'hidden',
  },
  // GroupBy step fields
  {
    label: '',
    name: 'groupByKey',
    initialValue: '',
    colSpan: 'full',
    type: 'hidden',
  },
  {
    label: '',
    name: 'groupByExpectedKeys',
    initialValue: [],
    colSpan: 'full',
    type: 'hidden',
  },
];

// Wrapper for InputMapping that only shows when Main tab is active
function InputMappingWrapper(config: Record<string, unknown>) {
  const { activeTab } = useTabContext();
  const form = useFormContext();
  const stepType = useWatch({ name: 'stepType', control: form.control });

  if (activeTab !== 'main') {
    return null;
  }

  if (stepType === 'Finish') {
    return <FinishStepField {...config} name={config.name as string} />;
  }

  if (stepType === 'Split') {
    return <SplitStepField {...config} name={config.name as string} />;
  }

  if (stepType === 'Error') {
    return <ErrorStepField {...config} name={config.name as string} />;
  }

  if (stepType === 'Filter') {
    return <FilterStepField {...config} name={config.name as string} />;
  }

  if (stepType === 'GroupBy') {
    return <GroupByStepField {...config} name={config.name as string} />;
  }

  if (stepType === 'AiAgent') {
    return <AiAgentStepField {...config} name={config.name as string} />;
  }

  if (stepType === 'WaitForSignal') {
    return <WaitForSignalStepField {...config} name={config.name as string} />;
  }

  if (stepType === 'Log') {
    return <LogStepField {...config} name={config.name as string} />;
  }

  if (stepType === 'While') {
    return <WhileStepField {...config} name={config.name as string} />;
  }

  if (stepType === 'Delay') {
    return <DelayStepField {...config} name={config.name as string} />;
  }

  return (
    <div className="-my-3">
      <InputMappingField {...config} />
    </div>
  );
}

// Main tab fields
const mainTabFieldsConfig = [
  {
    label: '',
    name: 'inputMapping',
    initialValue: [],
    description: '',
    colSpan: 'full',
    renderFormField: (config: Record<string, unknown>) => (
      <InputMappingWrapper {...config} />
    ),
  },
];

// Tab Provider Wrapper
export function TabProvider({ children }: { children: React.ReactNode }) {
  const [activeTab, setActiveTab] = useState('main');

  return (
    <TabContext.Provider value={{ activeTab, setActiveTab }}>
      {children}
    </TabContext.Provider>
  );
}

// Combined config for compatibility
export const fieldsConfig = [
  ...basicFieldsConfig,
  {
    type: 'custom',
    label: 'Configuration Tabs',
    name: 'formTabs',
    initialValue: undefined,
    colSpan: 'full',
    renderComponent: () => <FormTabs />,
  },
  ...mainTabFieldsConfig,
  {
    type: 'custom',
    label: '',
    name: 'stepAdvanced',
    initialValue: undefined,
    colSpan: 'full',
    renderComponent: () => <StepAdvancedFields />,
  },
];

export const schema = () =>
  z
    .object({
      name: z.string().nonempty(),
      stepType: z.string().nonempty(),
      agentId: z.string().optional(),
      capabilityId: z.string().optional(),
      connectionId: z.string().optional(),
      breakpoint: z.boolean().nullable().optional(),
      durable: z.boolean().nullable().optional(),
      timeout: z.any().optional(),
      compensation: z.any().optional(),
      onWait: z.any().optional(),
      action: z.any().optional(),
      childWorkflowId: z.string().optional(),
      // Pinned versions round-trip from the DSL as integers
      // (ChildVersion::Specific); coerce so a loaded pin doesn't fail
      // validation on a hidden field and silently dead-end the Save button.
      childVersion: z.coerce.string().optional(),
      embedWorkflowConfig: z.any().optional(), // UI-only field
      condition: z.any().optional(), // Condition for Conditional steps
      inputMapping: z.array(
        z
          .object({
            type: z.string(),
            value: z
              .union([
                z.string(),
                z.number(),
                z.boolean(),
                z.null(),
                z.array(z.any()),
                z.object({}).passthrough(),
              ])
              .optional(),
            typeHint: z.string().optional(),
            valueType: z
              .enum(['immediate', 'reference', 'composite', 'template'])
              .optional(),
            // ReferenceValue.default — fallback used at runtime when the
            // referenced path is missing or null. Must pass through the
            // resolver or a node-form save strips a JSON-authored default.
            defaultValue: z.any().optional(),
            // Editor-only marker for rows auto-seeded from a capability/child
            // workflow schema that the user never filled in. Must pass through
            // the resolver (zodResolver replaces form data with parsed output)
            // so the save path can drop untouched empty seeds while keeping
            // explicit immediate '' values.
            autoSeeded: z.boolean().optional(),
          })
          .refine(
            (item) => {
              // Validate JSON fields contain valid JSON strings
              // ('object'/'array' are form-level hints — e.g. Finish output
              // types — that carry the same JSON parse semantics on save)
              // BUT skip validation for:
              // - Reference values (they resolve at runtime)
              // - Template variables (they resolve at runtime)
              if (
                (item.typeHint === 'json' ||
                  item.typeHint === 'object' ||
                  item.typeHint === 'array') &&
                typeof item.value === 'string' &&
                item.value
              ) {
                // Skip validation for reference values - they're paths, not JSON
                if (item.valueType === 'reference') {
                  return true;
                }

                // Skip validation if value contains template syntax
                // Template variables like {{data.node.tags}} will be resolved at runtime
                if (item.value.includes('{{')) {
                  return true;
                }

                // For literal values, validate JSON syntax
                try {
                  JSON.parse(item.value);
                  return true;
                } catch {
                  return false;
                }
              }
              return true;
            },
            {
              message: 'Invalid JSON format',
              path: ['value'],
            }
          )
      ),
      executionTimeout: z.coerce.number().int().nonnegative(),
      maxRetries: z.coerce.number().int().nonnegative(),
      retryDelay: z.coerce.number().int().nonnegative(),
      retryStrategy: z.enum(['Linear', 'Exponential']).optional(),
      inputSchema: z.any().optional(),
      inputSchemaFields: z
        .array(
          z
            .object({
              name: z.string().optional(),
              type: z.string().optional(),
              required: z.boolean().optional(),
              description: z.string().optional(),
              defaultValue: z.any().optional(),
            })
            .passthrough()
        )
        .optional(),
      variablesFields: z
        .array(
          z.object({
            name: z.string().min(1, 'Variable name is required'),
            value: z.union([z.string(), z.number(), z.boolean(), z.any()]),
            type: z.string().optional(),
          })
        )
        .optional(),
      // Split step schema fields
      outputSchema: z.any().optional(),
      splitInputSchemaFields: z
        .array(
          z
            .object({
              name: z.string().optional(),
              type: z.string().optional(),
            })
            .passthrough()
        )
        .optional(),
      splitOutputSchemaFields: z
        .array(
          z
            .object({
              name: z.string().optional(),
              type: z.string().optional(),
            })
            .passthrough()
        )
        .optional(),
      // Split step config fields
      splitVariablesFields: z
        .array(
          z
            .object({
              name: z.string().optional(),
              value: z.any().optional(),
              type: z.string().optional(),
              // Must accept every mode MappingValueInput's toggle can cycle
              // into — omitting 'template' made the enum fail invisibly (no
              // rendered error for valueType) and Save silently no-op.
              valueType: z
                .enum(['reference', 'immediate', 'composite', 'template'])
                .optional(),
            })
            .superRefine((item, ctx) => {
              const trimmedName = (item.name || '').trim();
              const isComposite = item.valueType === 'composite';
              const isEmptyComposite =
                isComposite &&
                item.value !== undefined &&
                item.value !== null &&
                typeof item.value === 'object' &&
                (Array.isArray(item.value)
                  ? item.value.length === 0
                  : Object.keys(item.value as object).length === 0);
              const hasValue =
                item.value !== undefined &&
                (item.value !== null || item.valueType === 'immediate') &&
                !(typeof item.value === 'string' && item.value.trim() === '') &&
                !isEmptyComposite;

              if (hasValue && !trimmedName) {
                ctx.addIssue({
                  code: z.ZodIssueCode.custom,
                  path: ['name'],
                  message: 'Variable name is required',
                });
              }

              if (trimmedName && !hasValue) {
                ctx.addIssue({
                  code: z.ZodIssueCode.custom,
                  path: ['value'],
                  message: 'Variable value is required',
                });
              }
            })
        )
        .optional(),
      splitParallelism: z.number().optional(),
      splitSequential: z.boolean().optional(),
      splitDontStopOnFailed: z.boolean().optional(),
      splitMaxRetries: z.any().optional(),
      splitRetryDelay: z.any().optional(),
      splitTimeout: z.any().optional(),
      splitAllowNull: z.boolean().optional(),
      splitConvertSingleValue: z.boolean().optional(),
      splitBatchSize: z.any().optional(),
      // Filter step condition (stored separately from inputMapping)
      filterCondition: z.any().optional(),
      // While step fields
      whileCondition: z.any().optional(),
      whileMaxIterations: z.number().optional(),
      whileTimeout: z.number().nullable().optional(),
      // GroupBy step fields
      groupByKey: z.string().optional(),
      groupByExpectedKeys: z.array(z.string()).optional(),
    })
    .superRefine((inputs, ctx) => {
      // Switch requires a value to switch on: SwitchConfig.value is mandatory
      // on the backend (deny_unknown_fields serde struct), so an empty value
      // must block the form save instead of failing with a raw serde error.
      if (inputs.stepType !== 'Switch') return;
      const valueItem = (inputs.inputMapping || []).find(
        (item) => item.type === 'value'
      );
      const rawValue = valueItem?.value;
      const isEmptyComposite =
        valueItem?.valueType === 'composite' &&
        rawValue !== null &&
        typeof rawValue === 'object' &&
        (Array.isArray(rawValue)
          ? rawValue.length === 0
          : Object.keys(rawValue as object).length === 0);
      const hasValue =
        valueItem !== undefined &&
        rawValue !== undefined &&
        (rawValue !== null || valueItem.valueType === 'immediate') &&
        !(typeof rawValue === 'string' && rawValue.trim() === '') &&
        !isEmptyComposite;

      if (!hasValue) {
        ctx.addIssue({
          code: z.ZodIssueCode.custom,
          path: ['inputMapping'],
          message: 'Value to Switch On is required',
        });
      }
    })
    .superRefine((inputs, ctx) => {
      if (inputs.stepType !== 'Split') return;
      const splitVariables = inputs.splitVariablesFields || [];
      splitVariables.forEach((item, index) => {
        const trimmedName = (item.name || '').trim();
        const isComposite = item.valueType === 'composite';
        const isEmptyComposite =
          isComposite &&
          item.value !== undefined &&
          item.value !== null &&
          typeof item.value === 'object' &&
          (Array.isArray(item.value)
            ? item.value.length === 0
            : Object.keys(item.value as object).length === 0);
        const hasValue =
          item.value !== undefined &&
          (item.value !== null || item.valueType === 'immediate') &&
          !(typeof item.value === 'string' && item.value.trim() === '') &&
          !isEmptyComposite;

        if (hasValue && !trimmedName) {
          ctx.addIssue({
            code: z.ZodIssueCode.custom,
            path: ['splitVariablesFields', index, 'name'],
            message: 'Variable name is required',
          });
        }

        if (trimmedName && !hasValue) {
          ctx.addIssue({
            code: z.ZodIssueCode.custom,
            path: ['splitVariablesFields', index, 'value'],
            message: 'Variable value is required',
          });
        }
      });
    })
    .superRefine((inputs, ctx) => {
      // Finish outputs serialize through Object.fromEntries, which silently
      // collapses duplicate names last-wins before the server ever sees them.
      // Block the save with a visible error on every duplicated row instead.
      if (inputs.stepType !== 'Finish') return;
      const nameCounts = new Map<string, number>();
      (inputs.inputMapping || []).forEach((item) => {
        const trimmedName = (item.type || '').trim();
        if (!trimmedName) return;
        nameCounts.set(trimmedName, (nameCounts.get(trimmedName) || 0) + 1);
      });
      (inputs.inputMapping || []).forEach((item, index) => {
        const trimmedName = (item.type || '').trim();
        if (!trimmedName) return;
        if ((nameCounts.get(trimmedName) || 0) > 1) {
          ctx.addIssue({
            code: z.ZodIssueCode.custom,
            path: ['inputMapping', index, 'type'],
            message: `Duplicate output name "${trimmedName}"`,
          });
        }
      });
    });

export type SchemaType = z.infer<ReturnType<typeof schema>>;

export const initialValues: Partial<SchemaType> = {
  ...fieldsConfig.reduce((initValues: Record<string, any>, field) => {
    initValues[field.name] = field.initialValue;
    return initValues;
  }, {}),
  // Add advanced options default values
  executionTimeout: 120,
  maxRetries: 1,
  retryDelay: 1000,
  retryStrategy: 'Linear',
  breakpoint: undefined,
  durable: undefined,
  timeout: undefined,
  compensation: undefined,
  onWait: undefined,
  action: undefined,
  inputSchema: undefined,
  inputSchemaFields: [],
  variablesFields: [],
  // Condition for Conditional steps
  condition: undefined,
  // Split step schema fields
  outputSchema: undefined,
  splitInputSchemaFields: [],
  splitOutputSchemaFields: [],
  // Split step config fields
  splitVariablesFields: [],
  splitParallelism: 0,
  splitSequential: false,
  splitDontStopOnFailed: false,
  splitMaxRetries: undefined,
  splitRetryDelay: undefined,
  splitTimeout: undefined,
  splitAllowNull: false,
  splitConvertSingleValue: false,
  splitBatchSize: undefined,
  // GroupBy step fields
  groupByKey: '',
  groupByExpectedKeys: [],
};
