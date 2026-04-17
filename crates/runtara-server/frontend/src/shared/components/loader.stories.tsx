import type { Meta, StoryObj } from '@storybook/react';
import { Loader, Loader2 } from './loader';

const meta: Meta<typeof Loader> = {
  title: 'UI/Loader',
  component: Loader,
  parameters: {
    layout: 'fullscreen',
    docs: {
      description: {
        component:
          'Loading spinner components for indicating loading states. Loader is for full-page loading, Loader2 is for inline/section loading.',
      },
    },
  },
  tags: ['autodocs'],
};

export default meta;
type Story = StoryObj<typeof Loader>;

export const FullPage: Story = {
  name: 'Full Page Loader',
  render: () => (
    <div className="h-[400px] relative">
      <Loader />
    </div>
  ),
  parameters: {
    docs: {
      description: {
        story:
          'Full-page loader that centers a spinner in the viewport. Uses h-screen by default.',
      },
    },
  },
};

export const Inline: Story = {
  name: 'Inline Loader',
  render: () => (
    <div className="p-8">
      <Loader2 />
    </div>
  ),
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        story:
          'Inline loader for section-level loading states. Includes vertical margin (my-28).',
      },
    },
  },
};

export const InlineCustomSize: Story = {
  name: 'Inline Custom Size',
  render: () => (
    <div className="space-y-8 p-8">
      <div>
        <p className="text-xs text-muted-foreground mb-2">Small (h-4 w-4)</p>
        <Loader2 className="h-4 w-4 my-4" />
      </div>
      <div>
        <p className="text-xs text-muted-foreground mb-2">Default (h-8 w-8)</p>
        <Loader2 />
      </div>
      <div>
        <p className="text-xs text-muted-foreground mb-2">Large (h-12 w-12)</p>
        <Loader2 className="h-12 w-12 my-4" />
      </div>
      <div>
        <p className="text-xs text-muted-foreground mb-2">
          Extra Large (h-16 w-16)
        </p>
        <Loader2 className="h-16 w-16 my-4" />
      </div>
    </div>
  ),
  parameters: {
    layout: 'centered',
  },
};

export const InCard: Story = {
  name: 'In Card',
  render: () => (
    <div className="border rounded-lg p-6 w-[300px] h-[200px] flex items-center justify-center">
      <Loader2 className="my-0" />
    </div>
  ),
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        story:
          'Loader inside a card component. Override my-28 with my-0 for tighter spacing.',
      },
    },
  },
};

export const InButton: Story = {
  name: 'In Button (Pattern)',
  render: () => (
    <div className="space-y-4 p-8">
      <button
        disabled
        className="inline-flex items-center justify-center rounded-md bg-primary text-primary-foreground px-4 py-2 text-sm font-medium opacity-70"
      >
        <Loader2 className="h-4 w-4 my-0 mr-2" />
        Loading...
      </button>
      <button
        disabled
        className="inline-flex items-center justify-center rounded-md bg-secondary text-secondary-foreground px-4 py-2 text-sm font-medium opacity-70"
      >
        <Loader2 className="h-4 w-4 my-0 mr-2" />
        Processing...
      </button>
    </div>
  ),
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        story:
          'Pattern for showing a loader inside a button during async operations.',
      },
    },
  },
};

export const InTableRow: Story = {
  name: 'In Table Row',
  render: () => (
    <div className="border rounded-lg w-[400px]">
      <div className="p-3 border-b bg-muted/50">
        <span className="text-sm font-medium">Data Table</span>
      </div>
      <div className="p-8">
        <Loader2 className="my-4" />
      </div>
    </div>
  ),
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        story:
          'Pattern for showing a loader while table data is being fetched.',
      },
    },
  },
};

export const WithText: Story = {
  name: 'With Text',
  render: () => (
    <div className="flex flex-col items-center justify-center space-y-4 p-8">
      <Loader2 className="my-0" />
      <p className="text-sm text-muted-foreground">Loading your data...</p>
    </div>
  ),
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        story: 'Loader with descriptive text below it.',
      },
    },
  },
};

export const Comparison: Story = {
  name: 'Loader vs Loader2',
  render: () => (
    <div className="grid grid-cols-2 gap-8 p-8">
      <div className="border rounded-lg">
        <div className="p-3 border-b bg-muted/50">
          <span className="text-sm font-medium">Loader (Full Page)</span>
        </div>
        <div className="h-[200px] relative">
          <Loader />
        </div>
      </div>
      <div className="border rounded-lg">
        <div className="p-3 border-b bg-muted/50">
          <span className="text-sm font-medium">Loader2 (Inline)</span>
        </div>
        <div className="h-[200px] flex items-center justify-center">
          <Loader2 className="my-0" />
        </div>
      </div>
    </div>
  ),
  parameters: {
    layout: 'padded',
    docs: {
      description: {
        story:
          'Side-by-side comparison of Loader (full-page, uses h-screen) and Loader2 (inline, flexible margin).',
      },
    },
  },
};
