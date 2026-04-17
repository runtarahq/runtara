import type { Meta, StoryObj } from '@storybook/react';
import { Skeleton } from './skeleton';

const meta: Meta<typeof Skeleton> = {
  title: 'UI/Skeleton',
  component: Skeleton,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'A skeleton loading placeholder component. Use to indicate content is loading while maintaining layout structure.',
      },
    },
  },
  tags: ['autodocs'],
};

export default meta;
type Story = StoryObj<typeof Skeleton>;

export const Default: Story = {
  render: () => <Skeleton className="w-[200px] h-4" />,
};

export const Shapes: Story = {
  render: () => (
    <div className="space-y-4">
      <div>
        <p className="text-xs text-muted-foreground mb-2">Rectangle</p>
        <Skeleton className="w-[200px] h-4" />
      </div>
      <div>
        <p className="text-xs text-muted-foreground mb-2">Square</p>
        <Skeleton className="w-12 h-12" />
      </div>
      <div>
        <p className="text-xs text-muted-foreground mb-2">Circle (Avatar)</p>
        <Skeleton className="w-12 h-12 rounded-full" />
      </div>
      <div>
        <p className="text-xs text-muted-foreground mb-2">Rounded Rectangle</p>
        <Skeleton className="w-[200px] h-8 rounded-lg" />
      </div>
    </div>
  ),
};

export const TextLines: Story = {
  name: 'Text Lines',
  render: () => (
    <div className="space-y-2 w-[300px]">
      <Skeleton className="h-4 w-full" />
      <Skeleton className="h-4 w-full" />
      <Skeleton className="h-4 w-3/4" />
    </div>
  ),
};

export const CardSkeleton: Story = {
  name: 'Card Skeleton',
  render: () => (
    <div className="border rounded-lg p-4 w-[300px] space-y-4">
      <div className="flex items-center space-x-4">
        <Skeleton className="h-12 w-12 rounded-full" />
        <div className="space-y-2 flex-1">
          <Skeleton className="h-4 w-3/4" />
          <Skeleton className="h-3 w-1/2" />
        </div>
      </div>
      <div className="space-y-2">
        <Skeleton className="h-4 w-full" />
        <Skeleton className="h-4 w-full" />
        <Skeleton className="h-4 w-2/3" />
      </div>
    </div>
  ),
};

export const TableRowSkeleton: Story = {
  name: 'Table Row Skeleton',
  render: () => (
    <div className="w-[500px] border rounded-lg overflow-hidden">
      <div className="bg-muted/50 p-3 border-b">
        <div className="flex gap-4">
          <Skeleton className="h-4 w-[100px]" />
          <Skeleton className="h-4 w-[150px]" />
          <Skeleton className="h-4 w-[80px]" />
          <Skeleton className="h-4 w-[80px]" />
        </div>
      </div>
      {[1, 2, 3].map((row) => (
        <div key={row} className="p-3 border-b last:border-b-0">
          <div className="flex gap-4 items-center">
            <Skeleton className="h-4 w-[100px]" />
            <Skeleton className="h-4 w-[150px]" />
            <Skeleton className="h-4 w-[80px]" />
            <Skeleton className="h-6 w-[60px] rounded-full" />
          </div>
        </div>
      ))}
    </div>
  ),
};

export const ListSkeleton: Story = {
  name: 'List Skeleton',
  render: () => (
    <div className="space-y-3 w-[300px]">
      {[1, 2, 3, 4].map((item) => (
        <div key={item} className="flex items-center space-x-3">
          <Skeleton className="h-10 w-10 rounded" />
          <div className="space-y-1 flex-1">
            <Skeleton className="h-4 w-3/4" />
            <Skeleton className="h-3 w-1/2" />
          </div>
        </div>
      ))}
    </div>
  ),
};

export const FormSkeleton: Story = {
  name: 'Form Skeleton',
  render: () => (
    <div className="space-y-4 w-[350px]">
      <div className="space-y-2">
        <Skeleton className="h-4 w-[80px]" />
        <Skeleton className="h-10 w-full rounded-md" />
      </div>
      <div className="space-y-2">
        <Skeleton className="h-4 w-[100px]" />
        <Skeleton className="h-10 w-full rounded-md" />
      </div>
      <div className="space-y-2">
        <Skeleton className="h-4 w-[120px]" />
        <Skeleton className="h-24 w-full rounded-md" />
      </div>
      <Skeleton className="h-10 w-[120px] rounded-md" />
    </div>
  ),
};

export const DashboardSkeleton: Story = {
  name: 'Dashboard Cards Skeleton',
  render: () => (
    <div className="grid grid-cols-2 gap-4 w-[500px]">
      {[1, 2, 3, 4].map((card) => (
        <div key={card} className="border rounded-lg p-4 space-y-3">
          <Skeleton className="h-4 w-1/2" />
          <Skeleton className="h-8 w-3/4" />
          <Skeleton className="h-3 w-1/3" />
        </div>
      ))}
    </div>
  ),
};

export const ProfileSkeleton: Story = {
  name: 'Profile Skeleton',
  render: () => (
    <div className="flex flex-col items-center space-y-4 w-[250px]">
      <Skeleton className="h-24 w-24 rounded-full" />
      <div className="space-y-2 text-center w-full">
        <Skeleton className="h-5 w-2/3 mx-auto" />
        <Skeleton className="h-4 w-1/2 mx-auto" />
      </div>
      <div className="flex gap-4 w-full justify-center">
        <div className="text-center">
          <Skeleton className="h-5 w-8 mx-auto mb-1" />
          <Skeleton className="h-3 w-12" />
        </div>
        <div className="text-center">
          <Skeleton className="h-5 w-8 mx-auto mb-1" />
          <Skeleton className="h-3 w-12" />
        </div>
        <div className="text-center">
          <Skeleton className="h-5 w-8 mx-auto mb-1" />
          <Skeleton className="h-3 w-12" />
        </div>
      </div>
    </div>
  ),
};

export const Sizes: Story = {
  render: () => (
    <div className="space-y-4">
      <div>
        <p className="text-xs text-muted-foreground mb-2">Extra Small (h-2)</p>
        <Skeleton className="w-[200px] h-2" />
      </div>
      <div>
        <p className="text-xs text-muted-foreground mb-2">Small (h-3)</p>
        <Skeleton className="w-[200px] h-3" />
      </div>
      <div>
        <p className="text-xs text-muted-foreground mb-2">Default (h-4)</p>
        <Skeleton className="w-[200px] h-4" />
      </div>
      <div>
        <p className="text-xs text-muted-foreground mb-2">Medium (h-6)</p>
        <Skeleton className="w-[200px] h-6" />
      </div>
      <div>
        <p className="text-xs text-muted-foreground mb-2">Large (h-8)</p>
        <Skeleton className="w-[200px] h-8" />
      </div>
      <div>
        <p className="text-xs text-muted-foreground mb-2">Extra Large (h-12)</p>
        <Skeleton className="w-[200px] h-12" />
      </div>
    </div>
  ),
};

export const ImageSkeleton: Story = {
  name: 'Image Skeleton',
  render: () => (
    <div className="space-y-4">
      <div>
        <p className="text-xs text-muted-foreground mb-2">Thumbnail</p>
        <Skeleton className="w-16 h-16 rounded" />
      </div>
      <div>
        <p className="text-xs text-muted-foreground mb-2">Card Image</p>
        <Skeleton className="w-[300px] h-[200px] rounded-lg" />
      </div>
      <div>
        <p className="text-xs text-muted-foreground mb-2">Banner</p>
        <Skeleton className="w-[400px] h-[100px] rounded-lg" />
      </div>
    </div>
  ),
};
