import type { Meta, StoryObj } from '@storybook/react';
import { fn } from '@storybook/test';
import { useState } from 'react';
import { FileInput } from './file-input';

const meta: Meta<typeof FileInput> = {
  title: 'Forms/FileInput',
  component: FileInput,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'A file upload component with drag-and-drop support, file validation, loading states, and error handling. Files are encoded as base64 and stored as JSON.',
      },
    },
  },
  tags: ['autodocs'],
  argTypes: {
    value: {
      control: 'text',
      description: 'JSON string of FileData or empty string',
    },
    accept: {
      control: 'text',
      description: 'Accepted file types (e.g., ".pdf,.csv")',
    },
    disabled: {
      control: 'boolean',
      description: 'Disable the input',
    },
    placeholder: {
      control: 'text',
      description: 'Placeholder text',
    },
    error: {
      control: 'text',
      description: 'Error message to display',
    },
  },
  args: {
    onChange: fn(),
  },
  decorators: [
    (Story) => (
      <div className="w-[350px]">
        <Story />
      </div>
    ),
  ],
};

export default meta;
type Story = StoryObj<typeof FileInput>;

export const Default: Story = {
  args: {
    value: '',
  },
};

export const WithPlaceholder: Story = {
  name: 'Custom Placeholder',
  args: {
    value: '',
    placeholder: 'Drop your CSV file here',
  },
};

export const AcceptSpecificTypes: Story = {
  name: 'Accept Specific Types',
  args: {
    value: '',
    accept: '.pdf,.doc,.docx',
    placeholder: 'Upload PDF or Word document',
  },
};

export const AcceptImages: Story = {
  name: 'Accept Images Only',
  args: {
    value: '',
    accept: 'image/*',
    placeholder: 'Upload an image',
  },
};

// Simulated file data for demonstration
const mockFileData = JSON.stringify({
  filename: 'report-2024.pdf',
  mimeType: 'application/pdf',
  data: 'base64encodeddata...',
});

export const WithFile: Story = {
  name: 'With File Selected',
  args: {
    value: mockFileData,
  },
};

export const WithError: Story = {
  name: 'With Error',
  args: {
    value: '',
    error: 'File size exceeds the maximum limit of 10MB',
  },
};

export const Disabled: Story = {
  args: {
    value: '',
    disabled: true,
  },
};

export const DisabledWithFile: Story = {
  name: 'Disabled with File',
  args: {
    value: mockFileData,
    disabled: true,
  },
};

// Interactive example
const InteractiveExample = () => {
  const [value, setValue] = useState<string>('');

  return (
    <div className="space-y-4 w-[350px]">
      <FileInput
        value={value}
        onChange={setValue}
        accept=".pdf,.csv,.txt"
        placeholder="Upload PDF, CSV, or TXT file"
      />
      <div className="p-3 bg-slate-100 dark:bg-slate-800 rounded-lg">
        <p className="text-xs font-medium text-slate-500 dark:text-slate-400 mb-1">
          Current Value:
        </p>
        {value ? (
          <pre className="text-xs font-mono overflow-auto max-h-40">
            {JSON.stringify(JSON.parse(value), null, 2)}
          </pre>
        ) : (
          <p className="text-xs text-muted-foreground italic">
            No file selected
          </p>
        )}
      </div>
    </div>
  );
};

export const Interactive: Story = {
  render: () => <InteractiveExample />,
  parameters: {
    docs: {
      description: {
        story: 'Try uploading a file to see the JSON output.',
      },
    },
  },
};

export const AllStates: Story = {
  name: 'All States',
  render: () => (
    <div className="space-y-6 w-[350px]">
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          Empty State
        </p>
        <FileInput value="" onChange={() => {}} />
      </div>
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          With File
        </p>
        <FileInput value={mockFileData} onChange={() => {}} />
      </div>
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          With Error
        </p>
        <FileInput value="" onChange={() => {}} error="File too large" />
      </div>
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          Disabled
        </p>
        <FileInput value="" onChange={() => {}} disabled />
      </div>
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          Disabled with File
        </p>
        <FileInput value={mockFileData} onChange={() => {}} disabled />
      </div>
    </div>
  ),
};
