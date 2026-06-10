import { useState, useEffect } from 'react';
import { useForm } from 'react-hook-form';
import { z } from 'zod';
import { zodResolver } from '@hookform/resolvers/zod';
import { Settings, Variable, Download, Upload } from 'lucide-react';
import { cn } from '@/lib/utils';
import {
  Form,
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/shared/components/ui/form';
import { Input } from '@/shared/components/ui/input';
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
import type { MemoryTier } from '@/generated/RuntaraRuntimeApi';

type SettingsSection = 'general' | 'variables' | 'input' | 'output';

const sections: {
  id: SettingsSection;
  label: string;
  icon: React.ReactNode;
  description: string;
}[] = [
  {
    id: 'general',
    label: 'General',
    icon: <Settings className="h-4 w-4" />,
    description: 'Name, description, timeouts',
  },
  {
    id: 'variables',
    label: 'Variables',
    icon: <Variable className="h-4 w-4" />,
    description: 'Workflow constants',
  },
  {
    id: 'input',
    label: 'Input',
    icon: <Download className="h-4 w-4" />,
    description: 'Input schema fields',
  },
  {
    id: 'output',
    label: 'Output',
    icon: <Upload className="h-4 w-4" />,
    description: 'Output schema fields',
  },
];

const generalSchema = z.object({
  name: z.string().min(1, 'Workflow name is required'),
  description: z.string().optional(),
  entryPointId: z.string().optional(),
  executionTimeoutSeconds: z.coerce
    .number()
    .int()
    .min(1, 'Timeout must be at least 1 second')
    .max(3600, 'Timeout cannot exceed 3600 seconds (1 hour)')
    .optional(),
  rateLimitBudgetSec: z.coerce
    .number()
    .int()
    .min(1, 'Budget must be at least 1 second')
    .max(86400, 'Budget cannot exceed 86400 seconds (24 hours)')
    .optional(),
  durableMode: z.enum(['default', 'true', 'false']),
  memoryTier: z.enum(['default', 'S', 'M', 'L', 'XL']),
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
}

export function SettingsContent({
  workflow,
  onChange,
  readOnly = false,
}: SettingsContentProps) {
  const [activeSection, setActiveSection] =
    useState<SettingsSection>('general');

  return (
    <div className="flex flex-1 min-h-0 overflow-hidden">
      {/* Left panel - Sections list */}
      <div className="w-56 border-r flex-shrink-0 flex flex-col overflow-hidden">
        <div className="flex items-center px-3 py-1.5 border-b bg-muted/20">
          <span className="text-xs font-medium text-muted-foreground uppercase tracking-wider">
            Settings
          </span>
        </div>
        <div className="flex-1 overflow-y-auto">
          {sections.map((section) => {
            const isSelected = activeSection === section.id;

            return (
              <div
                key={section.id}
                className={cn(
                  'flex items-center gap-3 px-3 py-2.5 border-b cursor-pointer transition-colors',
                  'hover:bg-muted/50',
                  isSelected && 'bg-accent border-l-2 border-l-primary'
                )}
                onClick={() => setActiveSection(section.id)}
              >
                <div
                  className={cn(
                    'p-1.5 rounded',
                    isSelected
                      ? 'bg-primary/10 text-primary'
                      : 'bg-muted/50 text-muted-foreground'
                  )}
                >
                  {section.icon}
                </div>
                <div className="min-w-0 flex-1">
                  <div className="text-sm font-medium">{section.label}</div>
                  <div className="text-[10px] text-muted-foreground truncate">
                    {section.description}
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      </div>

      {/* Right panel - Section content */}
      <div className="flex-1 flex flex-col min-w-0 overflow-hidden">
        <div className="flex-1 overflow-y-auto p-4">
          {activeSection === 'general' && (
            <GeneralSection
              workflow={workflow}
              onChange={onChange}
              readOnly={readOnly}
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
        </div>
      </div>
    </div>
  );
}

function GeneralSection({
  workflow,
  onChange,
  readOnly,
}: {
  workflow: WorkflowData;
  onChange: (data: Partial<WorkflowData>) => void;
  readOnly: boolean;
}) {
  const form = useForm<GeneralFormValues>({
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
      memoryTier: workflow.memoryTier ?? 'default',
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
      memoryTier: workflow.memoryTier ?? 'default',
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
          memoryTier:
            values.memoryTier && values.memoryTier !== 'default'
              ? (values.memoryTier as MemoryTier)
              : null,
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
            name="memoryTier"
            render={({ field }) => (
              <FormItem className="w-40">
                <FormLabel>Memory tier</FormLabel>
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
                    <SelectItem value="S">S</SelectItem>
                    <SelectItem value="M">M</SelectItem>
                    <SelectItem value="L">L</SelectItem>
                    <SelectItem value="XL">XL</SelectItem>
                  </SelectContent>
                </Select>
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
