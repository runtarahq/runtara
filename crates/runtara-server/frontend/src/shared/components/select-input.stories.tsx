import type { Meta, StoryObj } from '@storybook/react';
import { useForm, FormProvider } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { z } from 'zod';
import { SelectInput } from './select-input';
import { Button } from './ui/button';
import { Form } from './ui/form';

const meta: Meta<typeof SelectInput> = {
  title: 'Forms/SelectInput',
  component: SelectInput,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'A form-integrated select dropdown component with label, description, and validation support. Requires React Hook Form context.',
      },
    },
  },
  tags: ['autodocs'],
  argTypes: {
    name: {
      control: 'text',
      description: 'Field name for form registration',
    },
    label: {
      control: 'text',
      description: 'Label text displayed above the select',
    },
    description: {
      control: 'text',
      description: 'Helper text displayed below the label',
    },
    disabled: {
      control: 'boolean',
      description: 'Disable the select',
    },
  },
};

export default meta;
type Story = StoryObj<typeof SelectInput>;

// Sample options
const countryOptions = [
  { value: 'us', label: 'United States' },
  { value: 'uk', label: 'United Kingdom' },
  { value: 'ca', label: 'Canada' },
  { value: 'au', label: 'Australia' },
  { value: 'de', label: 'Germany' },
  { value: 'fr', label: 'France' },
];

const priorityOptions = [
  { value: 'low', label: 'Low' },
  { value: 'medium', label: 'Medium' },
  { value: 'high', label: 'High' },
  { value: 'critical', label: 'Critical' },
];

const statusOptions = [
  { value: 'active', label: 'Active' },
  { value: 'inactive', label: 'Inactive' },
  { value: 'pending', label: 'Pending' },
  { value: 'archived', label: 'Archived', disabled: true },
];

// Form wrapper decorator
const FormWrapper = ({
  children,
  defaultValues = {},
}: {
  children: React.ReactNode;
  defaultValues?: Record<string, unknown>;
}) => {
  const form = useForm({
    defaultValues,
  });

  return (
    <FormProvider {...form}>
      <Form {...form}>
        <form className="space-y-4 w-[350px]">{children}</form>
      </Form>
    </FormProvider>
  );
};

export const Default: Story = {
  render: () => (
    <FormWrapper defaultValues={{ country: '' }}>
      <SelectInput name="country" label="Country" options={countryOptions} />
    </FormWrapper>
  ),
};

export const WithDescription: Story = {
  name: 'With Description',
  render: () => (
    <FormWrapper defaultValues={{ priority: '' }}>
      <SelectInput
        name="priority"
        label="Priority"
        options={priorityOptions}
        description="Select the priority level for this task"
      />
    </FormWrapper>
  ),
};

export const WithDefaultValue: Story = {
  name: 'With Default Value',
  render: () => (
    <FormWrapper defaultValues={{ country: 'us' }}>
      <SelectInput name="country" label="Country" options={countryOptions} />
    </FormWrapper>
  ),
};

export const Disabled: Story = {
  render: () => (
    <FormWrapper defaultValues={{ status: 'active' }}>
      <SelectInput
        name="status"
        label="Status"
        options={statusOptions}
        disabled
      />
    </FormWrapper>
  ),
};

export const WithDisabledOptions: Story = {
  name: 'With Disabled Options',
  render: () => (
    <FormWrapper defaultValues={{ status: '' }}>
      <SelectInput
        name="status"
        label="Status"
        options={statusOptions}
        description="Note: Archived option is disabled"
      />
    </FormWrapper>
  ),
};

export const NoLabel: Story = {
  name: 'Without Label',
  render: () => (
    <FormWrapper defaultValues={{ priority: '' }}>
      <SelectInput name="priority" options={priorityOptions} />
    </FormWrapper>
  ),
};

// Validation example
const validationSchema = z.object({
  country: z.string().min(1, 'Please select a country'),
  priority: z.string().min(1, 'Please select a priority'),
});

const ValidationExample = () => {
  const form = useForm({
    resolver: zodResolver(validationSchema),
    defaultValues: {
      country: '',
      priority: '',
    },
    mode: 'onBlur',
  });

  return (
    <FormProvider {...form}>
      <Form {...form}>
        <form
          onSubmit={form.handleSubmit((data) => console.log(data))}
          className="space-y-4 w-[350px]"
        >
          <SelectInput
            name="country"
            label="Country"
            options={countryOptions}
            description="Required field"
          />
          <SelectInput
            name="priority"
            label="Priority"
            options={priorityOptions}
            description="Required field"
          />
          <Button type="submit">Submit</Button>
          <p className="text-xs text-muted-foreground">
            Try submitting without selecting options to see validation errors.
          </p>
        </form>
      </Form>
    </FormProvider>
  );
};

export const WithValidation: Story = {
  name: 'With Validation',
  render: () => <ValidationExample />,
};

export const MultipleSelects: Story = {
  name: 'Multiple Selects',
  render: () => (
    <FormWrapper
      defaultValues={{
        country: '',
        priority: '',
        status: '',
      }}
    >
      <SelectInput name="country" label="Country" options={countryOptions} />
      <SelectInput name="priority" label="Priority" options={priorityOptions} />
      <SelectInput name="status" label="Status" options={statusOptions} />
    </FormWrapper>
  ),
};

export const CompleteFormExample: Story = {
  name: 'Complete Form Example',
  render: () => {
    const schema = z.object({
      projectType: z.string().min(1, 'Select a project type'),
      priority: z.string().min(1, 'Select a priority'),
      assignee: z.string().min(1, 'Select an assignee'),
    });

    const projectTypes = [
      { value: 'feature', label: 'Feature' },
      { value: 'bugfix', label: 'Bug Fix' },
      { value: 'improvement', label: 'Improvement' },
      { value: 'documentation', label: 'Documentation' },
    ];

    const assignees = [
      { value: 'john', label: 'John Doe' },
      { value: 'jane', label: 'Jane Smith' },
      { value: 'bob', label: 'Bob Wilson' },
      { value: 'alice', label: 'Alice Brown' },
    ];

    const FormExample = () => {
      const form = useForm({
        resolver: zodResolver(schema),
        defaultValues: {
          projectType: '',
          priority: '',
          assignee: '',
        },
        mode: 'onBlur',
      });

      return (
        <FormProvider {...form}>
          <Form {...form}>
            <form
              onSubmit={form.handleSubmit((data) =>
                alert(JSON.stringify(data, null, 2))
              )}
              className="space-y-4 w-[400px] p-4 border rounded-lg"
            >
              <h3 className="font-semibold text-lg">Create Task</h3>
              <SelectInput
                name="projectType"
                label="Project Type"
                options={projectTypes}
                description="Select the type of project"
              />
              <SelectInput
                name="priority"
                label="Priority"
                options={priorityOptions}
              />
              <SelectInput
                name="assignee"
                label="Assignee"
                options={assignees}
                description="Who will work on this task"
              />
              <div className="flex gap-2 pt-2">
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => form.reset()}
                >
                  Reset
                </Button>
                <Button type="submit">Create Task</Button>
              </div>
            </form>
          </Form>
        </FormProvider>
      );
    };

    return <FormExample />;
  },
};
