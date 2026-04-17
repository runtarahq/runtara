import type { Meta, StoryObj } from '@storybook/react';
import { useState, useEffect } from 'react';
import { Progress } from './progress';

const meta: Meta<typeof Progress> = {
  title: 'UI/Progress',
  component: Progress,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'A progress bar component built on Radix UI Progress. Shows completion status as a horizontal bar.',
      },
    },
  },
  tags: ['autodocs'],
  argTypes: {
    value: {
      control: { type: 'range', min: 0, max: 100 },
      description: 'Progress value (0-100)',
    },
  },
};

export default meta;
type Story = StoryObj<typeof Progress>;

export const Default: Story = {
  args: {
    value: 50,
  },
  render: (args) => <Progress {...args} className="w-[300px]" />,
};

export const Empty: Story = {
  args: {
    value: 0,
  },
  render: (args) => <Progress {...args} className="w-[300px]" />,
};

export const Full: Story = {
  args: {
    value: 100,
  },
  render: (args) => <Progress {...args} className="w-[300px]" />,
};

export const Values: Story = {
  name: 'Different Values',
  render: () => (
    <div className="space-y-4 w-[300px]">
      <div>
        <div className="flex justify-between text-xs text-muted-foreground mb-1">
          <span>0%</span>
        </div>
        <Progress value={0} />
      </div>
      <div>
        <div className="flex justify-between text-xs text-muted-foreground mb-1">
          <span>25%</span>
        </div>
        <Progress value={25} />
      </div>
      <div>
        <div className="flex justify-between text-xs text-muted-foreground mb-1">
          <span>50%</span>
        </div>
        <Progress value={50} />
      </div>
      <div>
        <div className="flex justify-between text-xs text-muted-foreground mb-1">
          <span>75%</span>
        </div>
        <Progress value={75} />
      </div>
      <div>
        <div className="flex justify-between text-xs text-muted-foreground mb-1">
          <span>100%</span>
        </div>
        <Progress value={100} />
      </div>
    </div>
  ),
};

export const WithLabel: Story = {
  name: 'With Label',
  render: () => (
    <div className="w-[300px] space-y-2">
      <div className="flex justify-between text-sm">
        <span>Uploading...</span>
        <span className="text-muted-foreground">67%</span>
      </div>
      <Progress value={67} />
    </div>
  ),
};

// Animated progress example
const AnimatedProgressExample = () => {
  const [progress, setProgress] = useState(0);

  useEffect(() => {
    const interval = setInterval(() => {
      setProgress((prev) => {
        if (prev >= 100) return 0;
        return prev + 5;
      });
    }, 200);

    return () => clearInterval(interval);
  }, []);

  return (
    <div className="w-[300px] space-y-2">
      <div className="flex justify-between text-sm">
        <span>Processing</span>
        <span className="text-muted-foreground">{progress}%</span>
      </div>
      <Progress value={progress} />
    </div>
  );
};

export const Animated: Story = {
  render: () => <AnimatedProgressExample />,
  parameters: {
    docs: {
      description: {
        story: 'Progress bar with animated value changes.',
      },
    },
  },
};

export const CustomHeight: Story = {
  name: 'Custom Height',
  render: () => (
    <div className="space-y-4 w-[300px]">
      <div>
        <p className="text-xs text-muted-foreground mb-2">Thin (h-1)</p>
        <Progress value={60} className="h-1" />
      </div>
      <div>
        <p className="text-xs text-muted-foreground mb-2">Default (h-2)</p>
        <Progress value={60} />
      </div>
      <div>
        <p className="text-xs text-muted-foreground mb-2">Medium (h-3)</p>
        <Progress value={60} className="h-3" />
      </div>
      <div>
        <p className="text-xs text-muted-foreground mb-2">Thick (h-4)</p>
        <Progress value={60} className="h-4" />
      </div>
    </div>
  ),
};

export const Widths: Story = {
  name: 'Different Widths',
  render: () => (
    <div className="space-y-4">
      <div>
        <p className="text-xs text-muted-foreground mb-2">Small (w-32)</p>
        <Progress value={60} className="w-32" />
      </div>
      <div>
        <p className="text-xs text-muted-foreground mb-2">Medium (w-64)</p>
        <Progress value={60} className="w-64" />
      </div>
      <div>
        <p className="text-xs text-muted-foreground mb-2">Large (w-96)</p>
        <Progress value={60} className="w-96" />
      </div>
      <div>
        <p className="text-xs text-muted-foreground mb-2">Full Width</p>
        <Progress value={60} className="w-full max-w-md" />
      </div>
    </div>
  ),
};

export const FileUpload: Story = {
  name: 'File Upload Pattern',
  render: () => (
    <div className="w-[350px] border rounded-lg p-4 space-y-3">
      <div className="flex items-center gap-3">
        <div className="w-10 h-10 bg-muted rounded flex items-center justify-center text-xs">
          PDF
        </div>
        <div className="flex-1 min-w-0">
          <p className="text-sm font-medium truncate">document.pdf</p>
          <p className="text-xs text-muted-foreground">2.4 MB</p>
        </div>
      </div>
      <div className="space-y-1">
        <div className="flex justify-between text-xs text-muted-foreground">
          <span>Uploading...</span>
          <span>78%</span>
        </div>
        <Progress value={78} />
      </div>
    </div>
  ),
};

export const MultipleFiles: Story = {
  name: 'Multiple Files Pattern',
  render: () => (
    <div className="w-[350px] space-y-3">
      <div className="border rounded-lg p-3">
        <div className="flex justify-between text-sm mb-2">
          <span className="truncate">image-001.jpg</span>
          <span className="text-green-600">Complete</span>
        </div>
        <Progress value={100} />
      </div>
      <div className="border rounded-lg p-3">
        <div className="flex justify-between text-sm mb-2">
          <span className="truncate">document.pdf</span>
          <span className="text-muted-foreground">45%</span>
        </div>
        <Progress value={45} />
      </div>
      <div className="border rounded-lg p-3">
        <div className="flex justify-between text-sm mb-2">
          <span className="truncate">video.mp4</span>
          <span className="text-muted-foreground">Waiting...</span>
        </div>
        <Progress value={0} />
      </div>
    </div>
  ),
};

export const StepProgress: Story = {
  name: 'Step Progress Pattern',
  render: () => (
    <div className="w-[400px] space-y-4">
      <div className="flex justify-between text-sm">
        <span>Step 2 of 4</span>
        <span className="text-muted-foreground">50%</span>
      </div>
      <Progress value={50} />
      <div className="flex justify-between text-xs text-muted-foreground">
        <span>Details</span>
        <span>Payment</span>
        <span>Review</span>
        <span>Complete</span>
      </div>
    </div>
  ),
};

export const QuotaUsage: Story = {
  name: 'Quota Usage Pattern',
  render: () => (
    <div className="w-[300px] space-y-4">
      <div className="space-y-2">
        <div className="flex justify-between text-sm">
          <span>Storage</span>
          <span className="text-muted-foreground">7.5 GB / 10 GB</span>
        </div>
        <Progress value={75} />
      </div>
      <div className="space-y-2">
        <div className="flex justify-between text-sm">
          <span>API Calls</span>
          <span className="text-muted-foreground">450 / 1000</span>
        </div>
        <Progress value={45} />
      </div>
      <div className="space-y-2">
        <div className="flex justify-between text-sm">
          <span>Bandwidth</span>
          <span className="text-red-600">95 GB / 100 GB</span>
        </div>
        <Progress value={95} />
      </div>
    </div>
  ),
};
