import type { Meta, StoryObj } from '@storybook/react';
import { useState } from 'react';
import { KeyValueInput } from './key-value-input';

const meta: Meta<typeof KeyValueInput> = {
  title: 'Forms/KeyValueInput',
  component: KeyValueInput,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'A key/value editor for `Record<string, string>` values. Use for backend fields typed as `HashMap<String, String>` (e.g. extra HTTP headers, per-tool hint maps). Press Enter or click + to add an entry; click X to remove. Empty keys are filtered out of the emitted value.',
      },
    },
  },
  tags: ['autodocs'],
  argTypes: {
    value: {
      control: 'object',
      description: 'Current value as a string→string map',
    },
    keyPlaceholder: {
      control: 'text',
      description: 'Placeholder for the key column',
    },
    valuePlaceholder: {
      control: 'text',
      description: 'Placeholder for the value column',
    },
  },
  decorators: [
    (Story) => (
      <div className="w-[480px]">
        <Story />
      </div>
    ),
  ],
};

export default meta;
type Story = StoryObj<typeof KeyValueInput>;

const KeyValueInputWrapper = ({
  initialValue = {},
  ...props
}: {
  initialValue?: Record<string, string>;
  keyPlaceholder?: string;
  valuePlaceholder?: string;
}) => {
  const [value, setValue] = useState<Record<string, string>>(initialValue);
  return <KeyValueInput value={value} onChange={setValue} {...props} />;
};

export const Empty: Story = {
  render: () => <KeyValueInputWrapper />,
};

export const ExtraHeaders: Story = {
  name: 'Extra HTTP Headers',
  render: () => (
    <KeyValueInputWrapper
      initialValue={{
        'X-Tenant': 'acme',
        'X-Request-ID': 'abc-123',
      }}
      keyPlaceholder="Header name"
      valuePlaceholder="Header value"
    />
  ),
};

export const ToolHints: Story = {
  name: 'Per-Tool Hints',
  render: () => (
    <KeyValueInputWrapper
      initialValue={{
        create_issue: 'Use when the user wants to file a ticket',
        search_issues: 'Use when the user asks about existing tickets',
      }}
      keyPlaceholder="Tool name"
      valuePlaceholder="Extra description"
    />
  ),
};

const InteractiveExample = () => {
  const [value, setValue] = useState<Record<string, string>>({
    foo: 'bar',
  });
  return (
    <div className="space-y-4 w-[480px]">
      <KeyValueInput
        value={value}
        onChange={setValue}
        keyPlaceholder="Key"
        valuePlaceholder="Value"
      />
      <div className="p-3 bg-slate-100 dark:bg-slate-800 rounded-lg">
        <p className="text-xs font-medium text-slate-500 dark:text-slate-400 mb-1">
          Current value:
        </p>
        <pre className="text-xs font-mono">
          {JSON.stringify(value, null, 2)}
        </pre>
      </div>
      <div className="text-xs text-muted-foreground space-y-1">
        <p>
          <kbd className="px-1 py-0.5 bg-muted rounded text-[10px]">Enter</kbd>{' '}
          - Commit the draft row
        </p>
        <p>
          <kbd className="px-1 py-0.5 bg-muted rounded text-[10px]">+</kbd> -
          Click to commit the draft row
        </p>
        <p>
          <kbd className="px-1 py-0.5 bg-muted rounded text-[10px]">X</kbd> -
          Remove an existing entry
        </p>
      </div>
    </div>
  );
};

export const Interactive: Story = {
  render: () => <InteractiveExample />,
  parameters: {
    docs: {
      description: {
        story:
          'Try adding and removing entries. Empty keys are filtered from the emitted value.',
      },
    },
  },
};
