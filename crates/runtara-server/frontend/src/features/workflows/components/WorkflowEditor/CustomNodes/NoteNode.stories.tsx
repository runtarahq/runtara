import type { Meta, StoryObj } from '@storybook/react';
import { Node } from '@xyflow/react';
import { NoteNode } from './NoteNode';
import { ReactFlowStoryWrapper, resetStores, setExecuting } from './storybook';
import { NODE_TYPES } from '@/features/workflows/config/workflow';

const NODE_ID = 'note-1';

// Helper to create a NoteNode
interface CreateNoteNodeOptions {
  content?: string;
  selected?: boolean;
  width?: number;
  height?: number;
}

function createNoteNode(opts: CreateNoteNodeOptions = {}): Node {
  const { content = '', selected = false, width = 240, height = 120 } = opts;

  return {
    id: NODE_ID,
    type: NODE_TYPES.NoteNode,
    position: { x: 50, y: 30 },
    selected,
    style: { width, height },
    data: {
      id: NODE_ID,
      content,
    },
  };
}

const meta: Meta<typeof NoteNode> = {
  title: 'WorkflowEditor/Nodes/NoteNode',
  component: NoteNode,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'Sticky note node for documentation. Supports Markdown formatting. Double-click to edit.',
      },
    },
  },
  tags: ['autodocs'],
};

export default meta;
type Story = StoryObj<typeof NoteNode>;

// Empty note
export const Empty: Story = {
  name: 'Empty (Placeholder)',
  decorators: [
    (Story) => {
      resetStores();
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper
      nodes={[createNoteNode()]}
      height={200}
      width={400}
    />
  ),
};

// With simple text
export const WithSimpleText: Story = {
  name: 'With Simple Text',
  decorators: [
    (Story) => {
      resetStores();
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper
      nodes={[
        createNoteNode({
          content:
            'This step processes incoming orders from the Shopify webhook.',
        }),
      ]}
      height={200}
      width={400}
    />
  ),
};

// With markdown content
export const WithMarkdown: Story = {
  name: 'With Markdown Content',
  decorators: [
    (Story) => {
      resetStores();
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper
      nodes={[
        createNoteNode({
          content: `## Order Processing

This section handles:
- **Order validation**
- **Inventory check**
- **Payment processing**

> Important: Always validate before processing!`,
          height: 200,
        }),
      ]}
      height={280}
      width={400}
    />
  ),
};

// Selected (shows resize handles and delete button)
export const Selected: Story = {
  decorators: [
    (Story) => {
      resetStores();
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper
      nodes={[
        createNoteNode({
          content: 'Selected note shows resize handles and delete button.',
          selected: true,
        }),
      ]}
      height={200}
      width={400}
    />
  ),
};

// Large note
export const LargeNote: Story = {
  name: 'Large Note',
  decorators: [
    (Story) => {
      resetStores();
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper
      nodes={[
        createNoteNode({
          content: `# Architecture Notes

## Data Flow
1. Webhook receives order
2. Validate payload schema
3. Check inventory availability
4. Process payment
5. Update order status
6. Send confirmation email

## Error Handling
- Retry failed API calls 3 times
- Log errors to monitoring service
- Notify on-call engineer for critical failures`,
          width: 350,
          height: 280,
        }),
      ]}
      height={380}
      width={500}
    />
  ),
};

// During execution (read-only)
export const DuringExecution: Story = {
  name: 'During Execution (Read-Only)',
  decorators: [
    (Story) => {
      resetStores();
      setExecuting();
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper
      nodes={[
        createNoteNode({
          content: 'Notes are read-only during execution.',
          selected: true,
        }),
      ]}
      height={200}
      width={400}
    />
  ),
};

// Multiple notes
export const MultipleNotes: Story = {
  name: 'Multiple Notes',
  parameters: {
    layout: 'padded',
  },
  decorators: [
    (Story) => {
      resetStores();
      return <Story />;
    },
  ],
  render: () => (
    <div className="grid grid-cols-2 gap-4">
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">Empty</p>
        <ReactFlowStoryWrapper
          nodes={[
            {
              id: 'note-empty',
              type: NODE_TYPES.NoteNode,
              position: { x: 50, y: 30 },
              style: { width: 240, height: 120 },
              data: { id: 'note-empty', content: '' },
            },
          ]}
          height={180}
          width={350}
        />
      </div>
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          With Text
        </p>
        <ReactFlowStoryWrapper
          nodes={[
            {
              id: 'note-text',
              type: NODE_TYPES.NoteNode,
              position: { x: 50, y: 30 },
              style: { width: 240, height: 120 },
              data: {
                id: 'note-text',
                content: 'Simple text note for documentation.',
              },
            },
          ]}
          height={180}
          width={350}
        />
      </div>
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          With Markdown
        </p>
        <ReactFlowStoryWrapper
          nodes={[
            {
              id: 'note-markdown',
              type: NODE_TYPES.NoteNode,
              position: { x: 50, y: 30 },
              style: { width: 240, height: 120 },
              data: {
                id: 'note-markdown',
                content:
                  '**Bold** and *italic* text\n- List item 1\n- List item 2',
              },
            },
          ]}
          height={180}
          width={350}
        />
      </div>
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          Selected
        </p>
        <ReactFlowStoryWrapper
          nodes={[
            {
              id: 'note-selected',
              type: NODE_TYPES.NoteNode,
              position: { x: 50, y: 30 },
              selected: true,
              style: { width: 240, height: 120 },
              data: {
                id: 'note-selected',
                content: 'Selected note shows controls.',
              },
            },
          ]}
          height={180}
          width={350}
        />
      </div>
    </div>
  ),
};
