import type { Meta, StoryObj } from '@storybook/react';
import { useForm, FormProvider } from 'react-hook-form';
import { StepTypeField } from './index';
import { Form } from '@/shared/components/ui/form';
import { StepTypeInfo } from '@/generated/RuntaraRuntimeApi';

// Mock step types
const mockStepTypes: StepTypeInfo[] = [
  {
    id: 'create',
    name: 'Create',
    description: 'Create a new record',
    category: 'data',
  },
  {
    id: 'agent',
    name: 'Agent',
    description: 'Execute an agent capability',
    category: 'action',
  },
  {
    id: 'conditional',
    name: 'Conditional',
    description: 'Branch based on conditions',
    category: 'flow',
  },
  {
    id: 'split',
    name: 'Split',
    description: 'Split into parallel branches',
    category: 'flow',
  },
  {
    id: 'switch',
    name: 'Switch',
    description: 'Route based on value',
    category: 'flow',
  },
  {
    id: 'combine',
    name: 'Combine',
    description: 'Merge parallel branches',
    category: 'flow',
  },
  {
    id: 'wait',
    name: 'Wait',
    description: 'Wait for time or event',
    category: 'timing',
  },
  {
    id: 'event',
    name: 'Event',
    description: 'Emit or wait for event',
    category: 'timing',
  },
  {
    id: 'terminate',
    name: 'Terminate',
    description: 'End the workflow',
    category: 'flow',
  },
];

const limitedStepTypes: StepTypeInfo[] = [
  {
    id: 'create',
    name: 'Create',
    description: 'Create a new record',
    category: 'data',
  },
  {
    id: 'agent',
    name: 'Agent',
    description: 'Execute an agent capability',
    category: 'action',
  },
  {
    id: 'conditional',
    name: 'Conditional',
    description: 'Branch based on conditions',
    category: 'flow',
  },
  {
    id: 'terminate',
    name: 'Terminate',
    description: 'End the workflow',
    category: 'flow',
  },
];

const meta: Meta<typeof StepTypeField> = {
  title: 'Scenarios/StepTypeField',
  component: StepTypeField,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'A grid-based step type selector for workflow nodes. Displays step types with icons and descriptions in a 2-column grid layout.',
      },
    },
  },
  tags: ['autodocs'],
};

export default meta;
type Story = StoryObj<typeof StepTypeField>;

// Form wrapper that provides stepTypes context
interface FormWrapperProps {
  children: React.ReactNode;
  defaultValue?: string;
  stepTypes?: StepTypeInfo[];
}

const FormWrapper = ({
  children,
  defaultValue = '',
  stepTypes = mockStepTypes,
}: FormWrapperProps) => {
  const form = useForm({
    defaultValues: {
      stepType: defaultValue,
      agentId: '',
      capabilityId: '',
    },
  });

  // Inject stepTypes into form context
  const extendedForm = {
    ...form,
    stepTypes,
  };

  return (
    <FormProvider {...extendedForm}>
      <Form {...form}>
        <form className="w-[400px]">{children}</form>
      </Form>
    </FormProvider>
  );
};

export const Default: Story = {
  render: () => (
    <FormWrapper>
      <StepTypeField
        name="stepType"
        label="Step Type"
        description="Select the type of step to add to your workflow"
      />
    </FormWrapper>
  ),
};

export const WithSelection: Story = {
  name: 'With Selection',
  render: () => (
    <FormWrapper defaultValue="Agent">
      <StepTypeField
        name="stepType"
        label="Step Type"
        description="Select the type of step to add to your workflow"
      />
    </FormWrapper>
  ),
};

export const CreateSelected: Story = {
  name: 'Create Selected',
  render: () => (
    <FormWrapper defaultValue="Create">
      <StepTypeField name="stepType" label="Step Type" />
    </FormWrapper>
  ),
};

export const ConditionalSelected: Story = {
  name: 'Conditional Selected',
  render: () => (
    <FormWrapper defaultValue="Conditional">
      <StepTypeField name="stepType" label="Step Type" />
    </FormWrapper>
  ),
};

export const LimitedOptions: Story = {
  name: 'Limited Options',
  render: () => (
    <FormWrapper stepTypes={limitedStepTypes}>
      <StepTypeField
        name="stepType"
        label="Step Type"
        description="Only basic step types available"
      />
    </FormWrapper>
  ),
};

export const NoLabel: Story = {
  name: 'Without Label',
  render: () => (
    <FormWrapper>
      <StepTypeField name="stepType" />
    </FormWrapper>
  ),
};

export const AllStepTypes: Story = {
  name: 'All Step Types Reference',
  render: () => (
    <div className="space-y-6 w-[500px]">
      <FormWrapper defaultValue="Create">
        <StepTypeField name="stepType" label="Create Step" />
      </FormWrapper>
      <FormWrapper defaultValue="Agent">
        <StepTypeField name="stepType" label="Agent Step" />
      </FormWrapper>
      <FormWrapper defaultValue="Conditional">
        <StepTypeField name="stepType" label="Conditional Step" />
      </FormWrapper>
      <FormWrapper defaultValue="Split">
        <StepTypeField name="stepType" label="Split Step" />
      </FormWrapper>
      <FormWrapper defaultValue="Combine">
        <StepTypeField name="stepType" label="Combine Step" />
      </FormWrapper>
    </div>
  ),
};

// Interactive example
const InteractiveExample = () => {
  const form = useForm({
    defaultValues: {
      stepType: '',
      agentId: '',
      capabilityId: '',
    },
  });

  const extendedForm = {
    ...form,
    stepTypes: mockStepTypes,
  };

  const selectedType = form.watch('stepType');

  return (
    <FormProvider {...extendedForm}>
      <Form {...form}>
        <div className="space-y-4 w-[400px]">
          <StepTypeField
            name="stepType"
            label="Step Type"
            description="Click a step type to select it"
          />
          <div className="p-3 bg-muted rounded text-sm">
            <strong>Selected:</strong>{' '}
            {selectedType || (
              <span className="text-muted-foreground">None</span>
            )}
          </div>
        </div>
      </Form>
    </FormProvider>
  );
};

export const Interactive: Story = {
  render: () => <InteractiveExample />,
  parameters: {
    docs: {
      description: {
        story: 'Click on different step types to see the selection change.',
      },
    },
  },
};
