import type { Meta, StoryObj } from '@storybook/react';
import { useForm, FormProvider } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { z } from 'zod';
import { CheckboxInput } from './checkbox-input';
import { TextInput } from './text-input';
import { Button } from './ui/button';
import { Form } from './ui/form';

const meta: Meta<typeof CheckboxInput> = {
  title: 'Forms/CheckboxInput',
  component: CheckboxInput,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'A switch-based checkbox component with label. Uses the Switch component under the hood for a modern toggle appearance.',
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
      description: 'Label text displayed next to the switch',
    },
    disabled: {
      control: 'boolean',
      description: 'Disable the switch',
    },
  },
};

export default meta;
type Story = StoryObj<typeof CheckboxInput>;

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
    <FormWrapper defaultValues={{ enabled: false }}>
      <CheckboxInput name="enabled" label="Enable feature" />
    </FormWrapper>
  ),
};

export const Checked: Story = {
  render: () => (
    <FormWrapper defaultValues={{ notifications: true }}>
      <CheckboxInput name="notifications" label="Enable notifications" />
    </FormWrapper>
  ),
};

export const Disabled: Story = {
  render: () => (
    <FormWrapper defaultValues={{ feature: false }}>
      <CheckboxInput name="feature" label="Disabled feature" disabled />
    </FormWrapper>
  ),
};

export const DisabledChecked: Story = {
  name: 'Disabled (Checked)',
  render: () => (
    <FormWrapper defaultValues={{ premium: true }}>
      <CheckboxInput name="premium" label="Premium feature (locked)" disabled />
    </FormWrapper>
  ),
};

export const MultipleCheckboxes: Story = {
  name: 'Multiple Checkboxes',
  render: () => (
    <FormWrapper
      defaultValues={{
        emailNotifications: true,
        pushNotifications: false,
        smsNotifications: false,
        marketingEmails: false,
      }}
    >
      <div className="space-y-4">
        <h3 className="font-semibold text-sm">Notification Preferences</h3>
        <CheckboxInput name="emailNotifications" label="Email notifications" />
        <CheckboxInput name="pushNotifications" label="Push notifications" />
        <CheckboxInput name="smsNotifications" label="SMS notifications" />
        <CheckboxInput name="marketingEmails" label="Marketing emails" />
      </div>
    </FormWrapper>
  ),
};

export const SettingsPanel: Story = {
  name: 'Settings Panel',
  render: () => (
    <FormWrapper
      defaultValues={{
        darkMode: false,
        compactView: false,
        autoSave: true,
        showHints: true,
        developerMode: false,
      }}
    >
      <div className="space-y-6 p-4 border rounded-lg w-[400px]">
        <div>
          <h3 className="font-semibold text-base mb-4">Display Settings</h3>
          <div className="space-y-4">
            <CheckboxInput name="darkMode" label="Dark mode" />
            <CheckboxInput name="compactView" label="Compact view" />
          </div>
        </div>
        <div className="border-t pt-4">
          <h3 className="font-semibold text-base mb-4">Behavior</h3>
          <div className="space-y-4">
            <CheckboxInput name="autoSave" label="Auto-save changes" />
            <CheckboxInput name="showHints" label="Show helpful hints" />
          </div>
        </div>
        <div className="border-t pt-4">
          <h3 className="font-semibold text-base mb-4">Advanced</h3>
          <div className="space-y-4">
            <CheckboxInput name="developerMode" label="Developer mode" />
          </div>
        </div>
      </div>
    </FormWrapper>
  ),
};

// Form with validation
const formSchema = z.object({
  name: z.string().min(1, 'Name is required'),
  termsAccepted: z.boolean().refine((val) => val === true, {
    message: 'You must accept the terms and conditions',
  }),
  newsletter: z.boolean().optional(),
});

const ValidationExample = () => {
  const form = useForm({
    resolver: zodResolver(formSchema),
    defaultValues: {
      name: '',
      termsAccepted: false,
      newsletter: false,
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
          <h3 className="font-semibold text-lg">Sign Up</h3>
          <TextInput name="name" label="Name" />
          <div className="space-y-3 pt-2">
            <CheckboxInput
              name="termsAccepted"
              label="I accept the terms and conditions"
            />
            <CheckboxInput
              name="newsletter"
              label="Subscribe to newsletter (optional)"
            />
          </div>
          <Button type="submit" className="w-full">
            Sign Up
          </Button>
          <p className="text-xs text-muted-foreground">
            Try submitting without accepting terms to see validation.
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

// Interactive state tracking
const InteractiveExample = () => {
  const form = useForm({
    defaultValues: {
      feature1: false,
      feature2: true,
      feature3: false,
    },
  });

  const values = form.watch();

  return (
    <FormProvider {...form}>
      <Form {...form}>
        <div className="space-y-4 w-[350px]">
          <div className="space-y-3">
            <CheckboxInput name="feature1" label="Feature A" />
            <CheckboxInput name="feature2" label="Feature B" />
            <CheckboxInput name="feature3" label="Feature C" />
          </div>
          <div className="p-3 bg-muted rounded text-sm">
            <strong>Current values:</strong>
            <pre className="mt-2 text-xs">
              {JSON.stringify(values, null, 2)}
            </pre>
          </div>
        </div>
      </Form>
    </FormProvider>
  );
};

export const InteractiveState: Story = {
  name: 'Interactive State Tracking',
  render: () => <InteractiveExample />,
};

export const PrivacySettings: Story = {
  name: 'Privacy Settings Example',
  render: () => {
    const FormExample = () => {
      const form = useForm({
        defaultValues: {
          analytics: true,
          personalizedAds: false,
          dataSelling: false,
          locationTracking: false,
          cookieConsent: true,
        },
      });

      return (
        <FormProvider {...form}>
          <Form {...form}>
            <form
              onSubmit={form.handleSubmit((data) =>
                alert('Settings saved!\n' + JSON.stringify(data, null, 2))
              )}
              className="space-y-4 w-[400px] p-4 border rounded-lg"
            >
              <div>
                <h3 className="font-semibold text-lg">Privacy Settings</h3>
                <p className="text-sm text-muted-foreground">
                  Control how your data is used
                </p>
              </div>

              <div className="space-y-4 py-2">
                <CheckboxInput
                  name="analytics"
                  label="Allow analytics cookies"
                />
                <CheckboxInput
                  name="personalizedAds"
                  label="Personalized advertisements"
                />
                <CheckboxInput
                  name="dataSelling"
                  label="Allow data selling to third parties"
                />
                <CheckboxInput
                  name="locationTracking"
                  label="Location-based services"
                />
                <CheckboxInput
                  name="cookieConsent"
                  label="Essential cookies (required)"
                  disabled
                />
              </div>

              <div className="flex gap-2 pt-2">
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => form.reset()}
                >
                  Reset to Defaults
                </Button>
                <Button type="submit">Save Preferences</Button>
              </div>
            </form>
          </Form>
        </FormProvider>
      );
    };

    return <FormExample />;
  },
};
