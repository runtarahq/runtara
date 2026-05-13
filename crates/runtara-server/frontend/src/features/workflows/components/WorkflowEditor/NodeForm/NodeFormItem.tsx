/* eslint-disable react-refresh/only-export-components */
// This file exports both components and configuration because the field configs
// contain JSX (renderFormField, renderComponent) which tightly couples them to components.
// Separating would require complex refactoring with circular dependency resolution.
import { z } from 'zod';
import { useState, createContext, useContext, useCallback } from 'react';
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
];

export const schema = () =>
  z
    .object({
      name: z.string().nonempty(),
      stepType: z.string().nonempty(),
      agentId: z.string().optional(),
      capabilityId: z.string().optional(),
      connectionId: z.string().optional(),
      childWorkflowId: z.string().optional(),
      childVersion: z.string().optional(),
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
                z.array(z.any()),
                z.object({}).passthrough(),
              ])
              .optional(),
            typeHint: z.string().optional(),
            valueType: z
              .enum(['immediate', 'reference', 'composite', 'template'])
              .optional(),
          })
          .refine(
            (item) => {
              // Validate JSON fields contain valid JSON strings
              // BUT skip validation for:
              // - Reference values (they resolve at runtime)
              // - Template variables (they resolve at runtime)
              if (
                item.typeHint === 'json' &&
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
          z.object({
            name: z.string().optional(),
            type: z.string().optional(),
            required: z.boolean().optional(),
            description: z.string().optional(),
            defaultValue: z.any().optional(),
          })
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
          z.object({
            name: z.string().optional(),
            type: z.string().optional(),
          })
        )
        .optional(),
      splitOutputSchemaFields: z
        .array(
          z.object({
            name: z.string().optional(),
            type: z.string().optional(),
          })
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
              valueType: z
                .enum(['reference', 'immediate', 'composite'])
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
                item.value !== null &&
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
          item.value !== null &&
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
  // GroupBy step fields
  groupByKey: '',
  groupByExpectedKeys: [],
};
