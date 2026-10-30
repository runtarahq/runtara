import { useState, useEffect } from 'react';
import { useForm } from 'react-hook-form';
import { z } from 'zod';
import { zodResolver } from '@hookform/resolvers/zod';
import { Settings, Variable, Download, Upload, Activity } from 'lucide-react';
import { cn } from '@/lib/utils';
import {
  Form,
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/shared/components/ui/form';
import { Input } from '@/shared/components/ui/input';
import { SectionLabel } from '@/shared/components/section-label';
import { Textarea } from '@/shared/components/ui/textarea';
import { Switch } from '@/shared/components/ui/switch';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { WorkflowData } from '../WorkflowEditor/EditorSidebar';
import {
  VariablesEditor,
  UIVariable,
} from '../WorkflowEditor/EditorSidebar/VariablesEditor';
import {
  SchemaFieldsEditor,
  SchemaField,
} from '../WorkflowEditor/EditorSidebar/SchemaFieldsEditor';
import { Button } from '@/shared/components/ui/button';
import {
  DEFAULT_MEMORY_MB,
  MAX_MEMORY_MB,
  MEMORY_PRESET_MIB,
} from '@/features/workflows/utils/memory';

type SettingsSection = 'general' | 'variables' | 'input' | 'output' | 'state';

const sections: {
  id: SettingsSection;
  label: string;
  icon: React.ReactNode;
  description: string;
}[] = [
  {
    id: 'general',
    label: 'General',
    icon: <Settings className="size-4" />,
    description: 'Name, description, timeouts',
  },
  {
    id: 'variables',
    label: 'Variables',
    icon: <Variable className="size-4" />,
    description: 'Workflow constants',
  },
  {
    id: 'input',
    label: 'Input',
    icon: <Download className="size-4" />,
    description: 'Input schema fields',
  },
  {
    id: 'output',
    label: 'Output',
    icon: <Upload className="size-4" />,
    description: 'Output schema fields',
  },
  {
    id: 'state',
    label: 'State',
    icon: <Activity className="size-4" />,
    description: 'State a run exposes',
  },
];

const generalSchema = z.object({
  name: z.string().min(1, 'Workflow name is required'),
  description: z.string().optional(),
  entryPointId: z.string().optional(),
  executionTimeoutSeconds: z.coerce
    .number<number>()
    .int()
    .min(1, 'Timeout must be at least 1 second')
    .max(3600, 'Timeout cannot exceed 3600 seconds (1 hour)')
    .optional(),
  rateLimitBudgetSec: z.coerce
    .number<number>()
    .int()
    .min(1, 'Budget must be at least 1 second')
    .max(86400, 'Budget cannot exceed 86400 seconds (24 hours)')
    .optional(),
  durableMode: z.enum(['default', 'true', 'false']),
  memoryMb: z.coerce
    .number<number>()
    .int()
    .min(1, 'Memory must be at least 1 MiB')
    .max(MAX_MEMORY_MB, `Memory cannot exceed ${MAX_MEMORY_MB} MiB`)
    .optional(),
  trackEvents: z.boolean(),
});

type GeneralFormValues = z.infer<typeof generalSchema>;

function durableToFormValue(durable?: boolean | null) {
  if (durable === true) return 'true';
  if (durable === false) return 'false';
  return 'default';
}

function formValueToDurable(value: GeneralFormValues['durableMode']) {
  if (value === 'true') return true;
  if (value === 'false') return false;
  return null;
}

const AUTO_ENTRY_POINT = '__auto__';

interface SettingsContentProps {
  workflow: WorkflowData;
  onChange: (data: Partial<WorkflowData>) => void;
  readOnly?: boolean;
  /** Linear memory the compiled workflow allocates when it starts. */
  memoryFloorBytes?: number | null;
}

export function SettingsContent({
  workflow,
  onChange,
  readOnly = false,
  memoryFloorBytes,
}: SettingsContentProps) {
  const [activeSection, setActiveSection] =
    useState<SettingsSection>('general');

  return (
    <div className="flex min-h-0 flex-1 overflow-hidden">
      {/* Left panel - Sections list */}
      <div className="flex w-56 flex-shrink-0 flex-col overflow-hidden border-r">
        <div className="flex items-center border-b bg-muted/20 px-3 py-1.5">
          <SectionLabel as="span">Settings</SectionLabel>
        </div>
        <div className="flex-1 overflow-y-auto">
          {sections.map((section) => {
            const isSelected = activeSection === section.id;

            return (
              <div
                key={section.id}
                className={cn(
                  'flex cursor-pointer items-center gap-3 border-b px-3 py-2.5 transition-colors',
                  'hover:bg-muted/50',
                  isSelected && 'border-l-2 border-l-primary bg-accent'
                )}
                onClick={() => setActiveSection(section.id)}
              >
                <div
                  className={cn(
                    'rounded p-1.5',
                    isSelected
                      ? 'bg-primary/10 text-primary-text'
                      : 'bg-muted/50 text-muted-foreground'
                  )}
                >
                  {section.icon}
                </div>
                <div className="min-w-0 flex-1">
                  <div className="text-sm font-medium">{section.label}</div>
                  <div className="truncate text-3xs text-muted-foreground">
                    {section.description}
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      </div>

      {/* Right panel - Section content */}
      <div className="flex min-w-0 flex-1 flex-col overflow-hidden">
        <div className="flex-1 overflow-y-auto p-4">
          {activeSection === 'general' && (
            <GeneralSection
              workflow={workflow}
              onChange={onChange}
              readOnly={readOnly}
              memoryFloorBytes={memoryFloorBytes}
            />
          )}
          {activeSection === 'variables' && (
            <VariablesSection
              variables={workflow.variables || []}
              onChange={(variables) => onChange({ variables })}
              readOnly={readOnly}
            />
          )}
          {activeSection === 'input' && (
            <InputSchemaSection
              fields={workflow.inputSchemaFields || []}
              onChange={(inputSchemaFields) => onChange({ inputSchemaFields })}
              readOnly={readOnly}
            />
          )}
          {activeSection === 'output' && (
            <OutputSchemaSection
              fields={workflow.outputSchemaFields || []}
              onChange={(outputSchemaFields) =>
                onChange({ outputSchemaFields })
              }
              readOnly={readOnly}
            />
          )}
          {activeSection === 'state' && (
            <StateSchemaSection
              fields={workflow.stateSchemaFields || []}
              onChange={(stateSchemaFields) => onChange({ stateSchemaFields })}
              readOnly={readOnly}
            />
          )}
        </div>
      </div>
    </div>
  );
}

function GeneralSection({
  workflow,
  onChange,
  readOnly,
  memoryFloorBytes,
}: {
  workflow: WorkflowData;
  onChange: (data: Partial<WorkflowData>) => void;
  readOnly: boolean;
  memoryFloorBytes?: number | null;
}) {
  const memoryFloorMib =
    memoryFloorBytes != null
      ? Math.ceil(memoryFloorBytes / (1024 * 1024))
      : null;
  const belowFloor = (mib: number) =>
    memoryFloorMib != null && mib < memoryFloorMib;
  const form = useForm<
    z.input<typeof generalSchema>,
    unknown,
    GeneralFormValues
  >({
    resolver: zodResolver(generalSchema),
    mode: 'onChange',
    defaultValues: {
      name: workflow.name || '',
      description: workflow.description || '',
      entryPointId: workflow.entryPoint || AUTO_ENTRY_POINT,
      executionTimeoutSeconds: workflow.executionTimeoutSeconds,
      rateLimitBudgetSec: workflow.rateLimitBudgetMs
        ? Math.round(workflow.rateLimitBudgetMs / 1000)
        : undefined,
      durableMode: durableToFormValue(workflow.durable),
      memoryMb: workflow.memoryMb ?? undefined,
      trackEvents: workflow.trackEvents ?? true,
    },
  });

  // Update form only when a different workflow is loaded (id changes)
  useEffect(() => {
    form.reset({
      name: workflow.name || '',
      description: workflow.description || '',
      entryPointId: workflow.entryPoint || AUTO_ENTRY_POINT,
      executionTimeoutSeconds: workflow.executionTimeoutSeconds,
      rateLimitBudgetSec: workflow.rateLimitBudgetMs
        ? Math.round(workflow.rateLimitBudgetMs / 1000)
        : undefined,
      durableMode: durableToFormValue(workflow.durable),
      memoryMb: workflow.memoryMb ?? undefined,
      trackEvents: workflow.trackEvents ?? true,
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workflow.id]);

  // Watch form values and sync changes to parent in real-time
  useEffect(() => {
    if (readOnly) return;

    const subscription = form.watch((values) => {
      // Only sync if name is valid (required field)
      // Note: We don't check form.formState.isValid here because it may not be
      // updated yet when the watch callback fires. Full validation happens at save time.
      if (values.name) {
        const timeout = values.executionTimeoutSeconds;
        const timeoutValue = timeout as unknown;
        const timeoutNumber =
          timeoutValue !== undefined && timeoutValue !== ''
            ? parseInt(String(timeoutValue), 10)
            : undefined;

        const budget = values.rateLimitBudgetSec;
        const budgetValue = budget as unknown;
        const budgetSeconds =
          budgetValue !== undefined && budgetValue !== ''
            ? parseInt(String(budgetValue), 10)
            : undefined;

        onChange({
          name: values.name,
          description: values.description || '',
          entryPoint:
            values.entryPointId && values.entryPointId !== AUTO_ENTRY_POINT
              ? values.entryPointId
              : undefined,
          executionTimeoutSeconds: timeoutNumber,
          rateLimitBudgetMs:
            budgetSeconds !== undefined ? budgetSeconds * 1000 : undefined,
          durable: formValueToDurable(values.durableMode ?? 'default'),
          memoryMb: (() => {
            const memory = values.memoryMb as unknown;
            return memory !== undefined && memory !== ''
              ? parseInt(String(memory), 10)
              : null;
          })(),
          trackEvents: values.trackEvents ?? true,
        });
      }
    });

    return () => subscription.unsubscribe();
  }, [form, onChange, readOnly]);

  return (
    <Form {...form}>
      <form className="flex flex-col gap-3">
        {/* Name and Timeout on the same row */}
        <div className="flex gap-4">
          <FormField
            control={form.control}
            name="name"
            render={({ field }) => (
              <FormItem className="flex-1">
                <FormLabel>Name</FormLabel>
                <FormControl>
                  <Input
                    {...field}
                    placeholder="Workflow name"
                    disabled={readOnly}
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />

          <FormField
            control={form.control}
            name="executionTimeoutSeconds"
            render={({ field }) => (
              <FormItem className="w-40">
                <FormLabel>Timeout (sec)</FormLabel>
                <FormControl>
                  <Input
                    {...field}
                    type="number"
                    min={1}
                    max={3600}
                    placeholder="300"
                    disabled={readOnly}
                    value={field.value ?? ''}
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />

          <FormField
            control={form.control}
            name="rateLimitBudgetSec"
            render={({ field }) => (
              <FormItem className="w-48">
                <FormLabel>Rate limit budget (sec)</FormLabel>
                <FormControl>
                  <Input
                    {...field}
                    type="number"
                    min={1}
                    max={86400}
                    placeholder="60"
                    disabled={readOnly}
                    value={field.value ?? ''}
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
        </div>

        <div className="flex gap-4">
          <FormField
            control={form.control}
            name="durableMode"
            render={({ field }) => (
              <FormItem className="w-44">
                <FormLabel>Durability</FormLabel>
                <Select
                  value={field.value ?? 'default'}
                  onValueChange={field.onChange}
                  disabled={readOnly}
                >
                  <FormControl>
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                  </FormControl>
                  <SelectContent>
                    <SelectItem value="default">Default</SelectItem>
                    <SelectItem value="true">Durable</SelectItem>
                    <SelectItem value="false">Non-durable</SelectItem>
                  </SelectContent>
                </Select>
                <FormMessage />
              </FormItem>
            )}
          />

          <FormField
            control={form.control}
            name="memoryMb"
            render={({ field }) => (
              <FormItem>
                <FormLabel>Memory (MiB)</FormLabel>
                <div className="flex items-center gap-1">
                  <FormControl>
                    <Input
                      {...field}
                      className="w-24"
                      type="number"
                      min={memoryFloorMib ?? 1}
                      max={MAX_MEMORY_MB}
                      placeholder={String(DEFAULT_MEMORY_MB)}
                      disabled={readOnly}
                      value={field.value ?? ''}
                    />
                  </FormControl>
                  {Object.entries(MEMORY_PRESET_MIB).map(([preset, mib]) => (
                    <Button
                      key={preset}
                      type="button"
                      variant="secondary"
                      size="sm"
                      title={`${mib} MiB`}
                      disabled={readOnly || belowFloor(mib)}
                      onClick={() => field.onChange(mib)}
                    >
                      {preset}
                    </Button>
                  ))}
                </div>
                {memoryFloorMib != null && (
                  <FormDescription>
                    Needs at least {memoryFloorMib} MiB
                  </FormDescription>
                )}
                {memoryFloorMib != null &&
                  field.value != null &&
                  String(field.value) !== '' &&
                  belowFloor(Number(field.value)) && (
                    <p className="text-xs text-destructive">
                      Below this workflow's floor: it will not compile
                    </p>
                  )}
                <FormMessage />
              </FormItem>
            )}
          />

          <FormField
            control={form.control}
            name="trackEvents"
            render={({ field }) => (
              <FormItem className="flex flex-1 items-center justify-between rounded-md border px-3 py-2">
                <FormLabel className="m-0">Track step events</FormLabel>
                <FormControl>
                  <Switch
                    checked={field.value}
                    onCheckedChange={field.onChange}
                    disabled={readOnly}
                  />
                </FormControl>
              </FormItem>
            )}
          />
        </div>

        <FormField
          control={form.control}
          name="entryPointId"
          render={({ field }) => (
            <FormItem>
              <FormLabel>Entry Point</FormLabel>
              <Select
                value={field.value || AUTO_ENTRY_POINT}
                onValueChange={field.onChange}
                disabled={readOnly}
              >
                <FormControl>
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                </FormControl>
                <SelectContent>
                  <SelectItem value={AUTO_ENTRY_POINT}>Auto</SelectItem>
                  {(workflow.entryPointOptions || []).map((step) => (
                    <SelectItem key={step.id} value={step.id}>
                      {step.name}
                    </SelectItem>
                  ))}
                  {workflow.entryPoint &&
                    !(workflow.entryPointOptions || []).some(
                      (step) => step.id === workflow.entryPoint
                    ) && (
                      <SelectItem value={workflow.entryPoint}>
                        {workflow.entryPoint}
                      </SelectItem>
                    )}
                </SelectContent>
              </Select>
              <FormMessage />
            </FormItem>
          )}
        />

        <FormField
          control={form.control}
          name="description"
          render={({ field }) => (
            <FormItem>
              <FormLabel>Description</FormLabel>
              <FormControl>
                <Textarea
                  {...field}
                  className="min-h-[60px] resize-none"
                  placeholder="Describe what this workflow does..."
                  disabled={readOnly}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
      </form>
    </Form>
  );
}

function VariablesSection({
  variables,
  onChange,
  readOnly,
}: {
  variables: UIVariable[];
  onChange: (variables: UIVariable[]) => void;
  readOnly: boolean;
}) {
  return (
    <VariablesEditor
      variables={variables}
      onChange={onChange}
      readOnly={readOnly}
      hideLabel
    />
  );
}

function InputSchemaSection({
  fields,
  onChange,
  readOnly,
}: {
  fields: SchemaField[];
  onChange: (fields: SchemaField[]) => void;
  readOnly: boolean;
}) {
  return (
    <SchemaFieldsEditor
      label="Input Schema Fields"
      fields={fields}
      onChange={onChange}
      readOnly={readOnly}
      emptyMessage="No input fields defined. Define the expected input parameters for this workflow."
      hideLabel
      showEnum
    />
  );
}

function OutputSchemaSection({
  fields,
  onChange,
  readOnly,
}: {
  fields: SchemaField[];
  onChange: (fields: SchemaField[]) => void;
  readOnly: boolean;
}) {
  return (
    <SchemaFieldsEditor
      label="Output Schema Fields"
      fields={fields}
      onChange={onChange}
      readOnly={readOnly}
      emptyMessage="No output fields defined. Define the expected output structure for this workflow."
      hideLabel
      showEnum
    />
  );
}

function StateSchemaSection({
  fields,
  onChange,
  readOnly,
}: {
  fields: SchemaField[];
  onChange: (fields: SchemaField[]) => void;
  readOnly: boolean;
}) {
  return (
    <div className="space-y-3">
      <p className="text-sm text-muted-foreground">
        Declare the typed state a run of this workflow exposes, with labels,
        formats and allowed values. State starts empty and is written by steps.
      </p>
      <SchemaFieldsEditor
        label="State schema"
        fields={fields}
        onChange={onChange}
        readOnly={readOnly}
        emptyMessage="No state fields defined. Declare the state a run exposes, such as its stage or amount."
        hideLabel
        showEnum
        mode="state"
      />
    </div>
  );
}
