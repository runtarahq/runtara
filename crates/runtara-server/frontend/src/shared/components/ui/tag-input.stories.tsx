import type { Meta, StoryObj } from '@storybook/react';
import { useState } from 'react';
import { TagInput } from './tag-input';

const meta: Meta<typeof TagInput> = {
  title: 'Forms/TagInput',
  component: TagInput,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'An input component for managing tags/chips. Press Enter to add a tag, Backspace to remove the last tag, or click the X button to remove specific tags.',
      },
    },
  },
  tags: ['autodocs'],
  argTypes: {
    value: {
      control: 'object',
      description: 'Array of tag strings',
    },
    placeholder: {
      control: 'text',
      description: 'Placeholder text when no tags exist',
    },
    className: {
      control: 'text',
      description: 'Additional CSS classes',
    },
  },
  decorators: [
    (Story) => (
      <div className="w-[400px]">
        <Story />
      </div>
    ),
  ],
};

export default meta;
type Story = StoryObj<typeof TagInput>;

// Interactive wrapper component
const TagInputWrapper = ({
  initialValue = [],
  ...props
}: {
  initialValue?: string[];
  placeholder?: string;
  className?: string;
}) => {
  const [value, setValue] = useState<string[]>(initialValue);
  return <TagInput value={value} onChange={setValue} {...props} />;
};

export const Default: Story = {
  render: () => <TagInputWrapper />,
};

export const WithPlaceholder: Story = {
  name: 'With Custom Placeholder',
  render: () => <TagInputWrapper placeholder="Add skills..." />,
};

export const WithInitialTags: Story = {
  name: 'With Initial Tags',
  render: () => (
    <TagInputWrapper initialValue={['React', 'TypeScript', 'Node.js']} />
  ),
};

export const ManyTags: Story = {
  name: 'Many Tags',
  render: () => (
    <TagInputWrapper
      initialValue={[
        'JavaScript',
        'TypeScript',
        'React',
        'Vue',
        'Angular',
        'Node.js',
        'Python',
        'Go',
        'Rust',
      ]}
    />
  ),
};

export const EmailTags: Story = {
  name: 'Email Tags Example',
  render: () => (
    <TagInputWrapper
      initialValue={['user@example.com', 'admin@company.com']}
      placeholder="Add email addresses..."
    />
  ),
};

export const CategoryTags: Story = {
  name: 'Category Tags',
  render: () => (
    <TagInputWrapper
      initialValue={['Featured', 'Sale', 'New Arrival']}
      placeholder="Add categories..."
    />
  ),
};

// Showcase with state display
const InteractiveExample = () => {
  const [value, setValue] = useState<string[]>(['initial', 'tags']);

  return (
    <div className="space-y-4 w-[400px]">
      <TagInput
        value={value}
        onChange={setValue}
        placeholder="Type and press Enter..."
      />
      <div className="p-3 bg-slate-100 dark:bg-slate-800 rounded-lg">
        <p className="text-xs font-medium text-slate-500 dark:text-slate-400 mb-1">
          Current Value:
        </p>
        <pre className="text-xs font-mono">
          {JSON.stringify(value, null, 2)}
        </pre>
      </div>
      <div className="text-xs text-muted-foreground space-y-1">
        <p>
          <kbd className="px-1 py-0.5 bg-muted rounded text-[10px]">Enter</kbd>{' '}
          - Add tag
        </p>
        <p>
          <kbd className="px-1 py-0.5 bg-muted rounded text-[10px]">
            Backspace
          </kbd>{' '}
          - Remove last tag (when input is empty)
        </p>
        <p>
          <kbd className="px-1 py-0.5 bg-muted rounded text-[10px]">X</kbd> -
          Click to remove specific tag
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
        story: 'Try adding and removing tags interactively.',
      },
    },
  },
};

export const LongTags: Story = {
  name: 'Long Tag Names',
  render: () => (
    <TagInputWrapper
      initialValue={[
        'Very Long Tag Name That Might Wrap',
        'Another Long Description Tag',
        'Short',
      ]}
    />
  ),
};

export const SingleTag: Story = {
  name: 'Single Tag',
  render: () => <TagInputWrapper initialValue={['Only one']} />,
};
