import type { Meta, StoryObj } from '@storybook/react';
import { useForm, FormProvider } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { z } from 'zod';
import { PasswordField } from './password-field';
import { TextInput } from './text-input';
import { Button } from './ui/button';
import { Form } from './ui/form';

const meta: Meta<typeof PasswordField> = {
  title: 'Forms/PasswordField',
  component: PasswordField,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'A password input component with visibility toggle. Built with form integration and validation support.',
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
  },
};

export default meta;
type Story = StoryObj<typeof PasswordField>;

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
    <FormWrapper defaultValues={{ password: '' }}>
      <PasswordField name="password" label="Password" onChange={() => {}} />
    </FormWrapper>
  ),
};

export const WithValue: Story = {
  name: 'With Value',
  render: () => (
    <FormWrapper defaultValues={{ password: 'secretpassword123' }}>
      <PasswordField name="password" label="Password" onChange={() => {}} />
    </FormWrapper>
  ),
  parameters: {
    docs: {
      description: {
        story: 'Click the eye icon to toggle password visibility.',
      },
    },
  },
};

export const NoLabel: Story = {
  name: 'Without Label',
  render: () => (
    <FormWrapper defaultValues={{ password: '' }}>
      <PasswordField name="password" onChange={() => {}} />
    </FormWrapper>
  ),
};

// Validation example
const passwordSchema = z.object({
  password: z
    .string()
    .min(8, 'Password must be at least 8 characters')
    .regex(/[A-Z]/, 'Password must contain at least one uppercase letter')
    .regex(/[a-z]/, 'Password must contain at least one lowercase letter')
    .regex(/[0-9]/, 'Password must contain at least one number'),
});

const ValidationExample = () => {
  const form = useForm({
    resolver: zodResolver(passwordSchema),
    defaultValues: {
      password: '',
    },
    mode: 'onBlur',
  });

  return (
    <FormProvider {...form}>
      <Form {...form}>
        <form
          onSubmit={form.handleSubmit((data) =>
            alert('Password: ' + data.password)
          )}
          className="space-y-4 w-[350px]"
        >
          <PasswordField name="password" label="Password" onChange={() => {}} />
          <div className="text-xs text-muted-foreground space-y-1">
            <p>Password requirements:</p>
            <ul className="list-disc list-inside">
              <li>At least 8 characters</li>
              <li>One uppercase letter</li>
              <li>One lowercase letter</li>
              <li>One number</li>
            </ul>
          </div>
          <Button type="submit">Submit</Button>
        </form>
      </Form>
    </FormProvider>
  );
};

export const WithValidation: Story = {
  name: 'With Validation',
  render: () => <ValidationExample />,
};

// Confirm password example
const confirmPasswordSchema = z
  .object({
    password: z.string().min(8, 'Password must be at least 8 characters'),
    confirmPassword: z.string(),
  })
  .refine((data) => data.password === data.confirmPassword, {
    message: "Passwords don't match",
    path: ['confirmPassword'],
  });

const ConfirmPasswordExample = () => {
  const form = useForm({
    resolver: zodResolver(confirmPasswordSchema),
    defaultValues: {
      password: '',
      confirmPassword: '',
    },
    mode: 'onBlur',
  });

  return (
    <FormProvider {...form}>
      <Form {...form}>
        <form
          onSubmit={form.handleSubmit(() => alert('Passwords match!'))}
          className="space-y-4 w-[350px]"
        >
          <PasswordField
            name="password"
            label="New Password"
            onChange={() => {}}
          />
          <PasswordField
            name="confirmPassword"
            label="Confirm Password"
            onChange={() => {}}
          />
          <Button type="submit">Update Password</Button>
        </form>
      </Form>
    </FormProvider>
  );
};

export const ConfirmPassword: Story = {
  name: 'Confirm Password',
  render: () => <ConfirmPasswordExample />,
  parameters: {
    docs: {
      description: {
        story:
          'Example of password confirmation pattern with matching validation.',
      },
    },
  },
};

// Login form example
const loginSchema = z.object({
  email: z.string().email('Invalid email'),
  password: z.string().min(1, 'Password is required'),
});

const LoginFormExample = () => {
  const form = useForm({
    resolver: zodResolver(loginSchema),
    defaultValues: {
      email: '',
      password: '',
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
          className="space-y-4 w-[350px] p-4 border rounded-lg"
        >
          <h3 className="font-semibold text-lg text-center">Sign In</h3>
          <TextInput name="email" label="Email" type="email" />
          <PasswordField name="password" label="Password" onChange={() => {}} />
          <Button type="submit" className="w-full">
            Sign In
          </Button>
        </form>
      </Form>
    </FormProvider>
  );
};

export const LoginForm: Story = {
  name: 'Login Form Example',
  render: () => <LoginFormExample />,
};

// Registration form example
const registerSchema = z
  .object({
    email: z.string().email('Invalid email'),
    password: z
      .string()
      .min(8, 'Password must be at least 8 characters')
      .regex(/[A-Z]/, 'Must contain uppercase')
      .regex(/[0-9]/, 'Must contain a number'),
    confirmPassword: z.string(),
  })
  .refine((data) => data.password === data.confirmPassword, {
    message: "Passwords don't match",
    path: ['confirmPassword'],
  });

const RegisterFormExample = () => {
  const form = useForm({
    resolver: zodResolver(registerSchema),
    defaultValues: {
      email: '',
      password: '',
      confirmPassword: '',
    },
    mode: 'onBlur',
  });

  return (
    <FormProvider {...form}>
      <Form {...form}>
        <form
          onSubmit={form.handleSubmit((data) =>
            alert('Registration successful!\n' + JSON.stringify(data, null, 2))
          )}
          className="space-y-4 w-[350px] p-4 border rounded-lg"
        >
          <h3 className="font-semibold text-lg text-center">Create Account</h3>
          <TextInput
            name="email"
            label="Email"
            type="email"
            description="We'll send a verification link"
          />
          <PasswordField name="password" label="Password" onChange={() => {}} />
          <PasswordField
            name="confirmPassword"
            label="Confirm Password"
            onChange={() => {}}
          />
          <Button type="submit" className="w-full">
            Create Account
          </Button>
        </form>
      </Form>
    </FormProvider>
  );
};

export const RegistrationForm: Story = {
  name: 'Registration Form Example',
  render: () => <RegisterFormExample />,
};
