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
import { ScenarioData } from '../WorkflowEditor/EditorSidebar';
import {
  VariablesEditor,
  UIVariable,
} from '../WorkflowEditor/EditorSidebar/VariablesEditor';
import {
  SchemaFieldsEditor,
  SchemaField,
} from '../WorkflowEditor/EditorSidebar/SchemaFieldsEditor';

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
    description: 'Scenario constants',
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
  name: z.string().min(1, 'Scenario name is required'),
  description: z.string().optional(),
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
});

type GeneralFormValues = z.infer<typeof generalSchema>;

interface SettingsContentProps {
  scenario: ScenarioData;
  onChange: (data: Partial<ScenarioData>) => void;
  readOnly?: boolean;
}

export function SettingsContent({
  scenario,
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
              scenario={scenario}
              onChange={onChange}
              readOnly={readOnly}
            />
          )}
          {activeSection === 'variables' && (
            <VariablesSection
              variables={scenario.variables || []}
              onChange={(variables) => onChange({ variables })}
              readOnly={readOnly}
            />
          )}
          {activeSection === 'input' && (
            <InputSchemaSection
              fields={scenario.inputSchemaFields || []}
              onChange={(inputSchemaFields) => onChange({ inputSchemaFields })}
              readOnly={readOnly}
            />
          )}
          {activeSection === 'output' && (
            <OutputSchemaSection
              fields={scenario.outputSchemaFields || []}
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
  scenario,
  onChange,
  readOnly,
}: {
  scenario: ScenarioData;
  onChange: (data: Partial<ScenarioData>) => void;
  readOnly: boolean;
}) {
  const form = useForm<GeneralFormValues>({
    resolver: zodResolver(generalSchema),
    mode: 'onChange',
    defaultValues: {
      name: scenario.name || '',
      description: scenario.description || '',
      executionTimeoutSeconds: scenario.executionTimeoutSeconds,
      rateLimitBudgetSec: scenario.rateLimitBudgetMs
        ? Math.round(scenario.rateLimitBudgetMs / 1000)
        : undefined,
    },
  });

  // Update form only when a different scenario is loaded (id changes)
  useEffect(() => {
    form.reset({
      name: scenario.name || '',
      description: scenario.description || '',
      executionTimeoutSeconds: scenario.executionTimeoutSeconds,
      rateLimitBudgetSec: scenario.rateLimitBudgetMs
        ? Math.round(scenario.rateLimitBudgetMs / 1000)
        : undefined,
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scenario.id]);

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
          executionTimeoutSeconds: timeoutNumber,
          rateLimitBudgetMs:
            budgetSeconds !== undefined ? budgetSeconds * 1000 : undefined,
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
                    placeholder="Scenario name"
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
                  placeholder="Describe what this scenario does..."
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
      emptyMessage="No input fields defined. Define the expected input parameters for this scenario."
      hideLabel
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
      emptyMessage="No output fields defined. Define the expected output structure for this scenario."
      hideLabel
    />
  );
}
