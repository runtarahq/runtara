import type { Meta, StoryObj } from '@storybook/react';
import { useForm, FormProvider } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { z } from 'zod';
import { TextInput } from './text-input';
import { Button } from './ui/button';
import { Form } from './ui/form';

const meta: Meta<typeof TextInput> = {
  title: 'Forms/TextInput',
  component: TextInput,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'A form-integrated text input component with label, description, and validation support. Requires React Hook Form context.',
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
      description: 'Label text displayed above the input',
    },
    type: {
      control: 'select',
      options: ['text', 'email', 'password', 'number', 'tel', 'url'],
      description: 'Input type',
    },
    description: {
      control: 'text',
      description: 'Helper text displayed below the label',
    },
    showError: {
      control: 'boolean',
      description: 'Whether to show validation errors',
    },
  },
};

export default meta;
type Story = StoryObj<typeof TextInput>;

// Form wrapper decorator
const FormWrapper = ({
  children,
  defaultValues = {},
  onSubmit,
}: {
  children: React.ReactNode;
  defaultValues?: Record<string, unknown>;
  onSubmit?: (data: Record<string, unknown>) => void;
}) => {
  const form = useForm({
    defaultValues,
  });

  return (
    <FormProvider {...form}>
      <Form {...form}>
        <form
          onSubmit={form.handleSubmit(onSubmit || (() => {}))}
          className="space-y-4 w-[350px]"
        >
          {children}
        </form>
      </Form>
    </FormProvider>
  );
};

export const Default: Story = {
  render: () => (
    <FormWrapper defaultValues={{ username: '' }}>
      <TextInput name="username" label="Username" />
    </FormWrapper>
  ),
};

export const WithDescription: Story = {
  name: 'With Description',
  render: () => (
    <FormWrapper defaultValues={{ email: '' }}>
      <TextInput
        name="email"
        label="Email"
        type="email"
        description="We'll never share your email with anyone else."
      />
    </FormWrapper>
  ),
};

export const WithDefaultValue: Story = {
  name: 'With Default Value',
  render: () => (
    <FormWrapper defaultValues={{ name: 'John Doe' }}>
      <TextInput name="name" label="Full Name" />
    </FormWrapper>
  ),
};

export const NoLabel: Story = {
  name: 'Without Label',
  render: () => (
    <FormWrapper defaultValues={{ search: '' }}>
      <TextInput name="search" />
    </FormWrapper>
  ),
};

export const InputTypes: Story = {
  name: 'Different Types',
  render: () => (
    <FormWrapper
      defaultValues={{
        text: '',
        email: '',
        number: '',
        tel: '',
        url: '',
      }}
    >
      <TextInput name="text" label="Text" type="text" />
      <TextInput name="email" label="Email" type="email" />
      <TextInput name="number" label="Number" type="number" />
      <TextInput name="tel" label="Phone" type="tel" />
      <TextInput name="url" label="URL" type="url" />
    </FormWrapper>
  ),
};

// Validation example
const validationSchema = z.object({
  email: z.string().email('Please enter a valid email address'),
  username: z.string().min(3, 'Username must be at least 3 characters'),
});

const ValidationExample = () => {
  const form = useForm({
    resolver: zodResolver(validationSchema),
    defaultValues: {
      email: '',
      username: '',
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
          <TextInput
            name="email"
            label="Email"
            type="email"
            description="Enter a valid email address"
          />
          <TextInput
            name="username"
            label="Username"
            description="Must be at least 3 characters"
          />
          <Button type="submit">Submit</Button>
          <p className="text-xs text-muted-foreground">
            Try submitting with invalid data to see validation errors.
          </p>
        </form>
      </Form>
    </FormProvider>
  );
};

export const WithValidation: Story = {
  name: 'With Validation',
  render: () => <ValidationExample />,
  parameters: {
    docs: {
      description: {
        story:
          'Form with Zod validation. Blur fields or submit to see validation errors.',
      },
    },
  },
};

export const HideErrors: Story = {
  name: 'Hide Errors',
  render: () => (
    <FormWrapper defaultValues={{ field: '' }}>
      <TextInput
        name="field"
        label="Field with showError=false"
        showError={false}
        description="Validation errors will not be displayed"
      />
    </FormWrapper>
  ),
};

export const CompleteForm: Story = {
  name: 'Complete Form Example',
  render: () => {
    const schema = z.object({
      firstName: z.string().min(1, 'First name is required'),
      lastName: z.string().min(1, 'Last name is required'),
      email: z.string().email('Invalid email'),
      phone: z.string().optional(),
      website: z.string().url('Invalid URL').optional().or(z.literal('')),
    });

    const FormExample = () => {
      const form = useForm({
        resolver: zodResolver(schema),
        defaultValues: {
          firstName: '',
          lastName: '',
          email: '',
          phone: '',
          website: '',
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
              <div className="grid grid-cols-2 gap-4">
                <TextInput name="firstName" label="First Name" />
                <TextInput name="lastName" label="Last Name" />
              </div>
              <TextInput
                name="email"
                label="Email"
                type="email"
                description="Your primary email address"
              />
              <TextInput name="phone" label="Phone (Optional)" type="tel" />
              <TextInput name="website" label="Website (Optional)" type="url" />
              <div className="flex gap-2 pt-2">
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => form.reset()}
                >
                  Reset
                </Button>
                <Button type="submit">Submit</Button>
              </div>
            </form>
          </Form>
        </FormProvider>
      );
    };

    return <FormExample />;
  },
};
