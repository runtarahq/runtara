import type { Meta, StoryObj } from '@storybook/react';
import { CopyIdButton } from './copy-id-button';

const meta: Meta<typeof CopyIdButton> = {
  title: 'Shared/CopyIdButton',
  component: CopyIdButton,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'A button component for copying IDs to clipboard. Shows a key icon and displays a success toast when clicked.',
      },
    },
  },
  tags: ['autodocs'],
  argTypes: {
    variant: {
      control: 'select',
      options: [
        'default',
        'outline',
        'secondary',
        'ghost',
        'link',
        'destructive',
      ],
      description: 'Button variant style',
    },
    size: {
      control: 'select',
      options: ['default', 'sm', 'lg', 'icon'],
      description: 'Button size',
    },
  },
};

export default meta;
type Story = StoryObj<typeof CopyIdButton>;

export const Default: Story = {
  args: {
    id: 'abc-123-def-456',
  },
};

export const IconOnly: Story = {
  name: 'Icon Only (Default)',
  args: {
    id: 'scn-001-xyz-789',
    size: 'icon',
    variant: 'ghost',
  },
};

export const WithLabel: Story = {
  name: 'With Label',
  args: {
    id: 'conn-abc-123',
    size: 'sm',
    variant: 'outline',
  },
};

export const OutlineVariant: Story = {
  name: 'Outline Variant',
  args: {
    id: 'trigger-xyz-456',
    variant: 'outline',
    size: 'icon',
  },
};

export const SecondaryVariant: Story = {
  name: 'Secondary Variant',
  args: {
    id: 'obj-789-abc',
    variant: 'secondary',
    size: 'icon',
  },
};

export const AllVariants: Story = {
  name: 'All Variants Reference',
  render: () => (
    <div className="flex flex-col gap-4">
      <div className="flex items-center gap-4">
        <span className="text-sm text-muted-foreground w-24">Ghost:</span>
        <CopyIdButton id="ghost-id" variant="ghost" size="icon" />
      </div>
      <div className="flex items-center gap-4">
        <span className="text-sm text-muted-foreground w-24">Outline:</span>
        <CopyIdButton id="outline-id" variant="outline" size="icon" />
      </div>
      <div className="flex items-center gap-4">
        <span className="text-sm text-muted-foreground w-24">Secondary:</span>
        <CopyIdButton id="secondary-id" variant="secondary" size="icon" />
      </div>
      <div className="flex items-center gap-4">
        <span className="text-sm text-muted-foreground w-24">Default:</span>
        <CopyIdButton id="default-id" variant="default" size="icon" />
      </div>
    </div>
  ),
};

export const AllSizes: Story = {
  name: 'All Sizes Reference',
  render: () => (
    <div className="flex items-center gap-4">
      <div className="text-center">
        <CopyIdButton id="icon-size" size="icon" />
        <p className="text-xs text-muted-foreground mt-1">icon</p>
      </div>
      <div className="text-center">
        <CopyIdButton id="sm-size" size="sm" />
        <p className="text-xs text-muted-foreground mt-1">sm</p>
      </div>
      <div className="text-center">
        <CopyIdButton id="default-size" size="default" />
        <p className="text-xs text-muted-foreground mt-1">default</p>
      </div>
      <div className="text-center">
        <CopyIdButton id="lg-size" size="lg" />
        <p className="text-xs text-muted-foreground mt-1">lg</p>
      </div>
    </div>
  ),
};
