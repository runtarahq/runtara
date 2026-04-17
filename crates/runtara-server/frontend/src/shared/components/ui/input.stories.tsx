import type { Meta, StoryObj } from '@storybook/react';
import { Input } from './input';
import { Label } from './label';
import {
  Search,
  Mail,
  Lock,
  Eye,
  EyeOff,
  Calendar,
  DollarSign,
} from 'lucide-react';
import { useState } from 'react';

const meta: Meta<typeof Input> = {
  title: 'UI/Input',
  component: Input,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'A basic input component that wraps the native HTML input element with consistent styling. For form integration with labels and validation, use TextInput instead.',
      },
    },
  },
  tags: ['autodocs'],
  argTypes: {
    type: {
      control: 'select',
      options: [
        'text',
        'email',
        'password',
        'number',
        'search',
        'tel',
        'url',
        'date',
        'time',
      ],
      description: 'Input type',
    },
    placeholder: {
      control: 'text',
      description: 'Placeholder text',
    },
    disabled: {
      control: 'boolean',
      description: 'Disabled state',
    },
  },
};

export default meta;
type Story = StoryObj<typeof Input>;

export const Default: Story = {
  args: {
    type: 'text',
    placeholder: 'Enter text...',
  },
};

export const WithLabel: Story = {
  name: 'With Label',
  render: () => (
    <div className="grid w-full max-w-sm items-center gap-1.5">
      <Label htmlFor="email">Email</Label>
      <Input type="email" id="email" placeholder="Enter your email" />
    </div>
  ),
};

export const Disabled: Story = {
  args: {
    type: 'text',
    placeholder: 'Disabled input',
    disabled: true,
  },
};

export const WithValue: Story = {
  name: 'With Value',
  args: {
    type: 'text',
    defaultValue: 'Pre-filled value',
  },
};

export const InputTypes: Story = {
  name: 'Input Types',
  render: () => (
    <div className="space-y-4 w-[300px]">
      <div className="space-y-1.5">
        <Label>Text</Label>
        <Input type="text" placeholder="Enter text" />
      </div>
      <div className="space-y-1.5">
        <Label>Email</Label>
        <Input type="email" placeholder="email@example.com" />
      </div>
      <div className="space-y-1.5">
        <Label>Password</Label>
        <Input type="password" placeholder="Enter password" />
      </div>
      <div className="space-y-1.5">
        <Label>Number</Label>
        <Input type="number" placeholder="0" />
      </div>
      <div className="space-y-1.5">
        <Label>Date</Label>
        <Input type="date" />
      </div>
      <div className="space-y-1.5">
        <Label>Time</Label>
        <Input type="time" />
      </div>
      <div className="space-y-1.5">
        <Label>URL</Label>
        <Input type="url" placeholder="https://example.com" />
      </div>
      <div className="space-y-1.5">
        <Label>Tel</Label>
        <Input type="tel" placeholder="+1 (555) 000-0000" />
      </div>
    </div>
  ),
};

export const WithIconLeft: Story = {
  name: 'With Icon (Left)',
  render: () => (
    <div className="space-y-4 w-[300px]">
      <div className="relative">
        <Search className="absolute left-2.5 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
        <Input type="search" placeholder="Search..." className="pl-8" />
      </div>
      <div className="relative">
        <Mail className="absolute left-2.5 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
        <Input type="email" placeholder="Email address" className="pl-8" />
      </div>
      <div className="relative">
        <DollarSign className="absolute left-2.5 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
        <Input type="number" placeholder="0.00" className="pl-8" />
      </div>
    </div>
  ),
};

export const PasswordToggle: Story = {
  name: 'Password with Toggle',
  render: function PasswordToggleStory() {
    const [showPassword, setShowPassword] = useState(false);
    return (
      <div className="relative w-[300px]">
        <Lock className="absolute left-2.5 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
        <Input
          type={showPassword ? 'text' : 'password'}
          placeholder="Enter password"
          className="pl-8 pr-10"
        />
        <button
          type="button"
          onClick={() => setShowPassword(!showPassword)}
          className="absolute right-2.5 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground"
        >
          {showPassword ? (
            <EyeOff className="h-4 w-4" />
          ) : (
            <Eye className="h-4 w-4" />
          )}
        </button>
      </div>
    );
  },
};

export const States: Story = {
  name: 'States',
  render: () => (
    <div className="space-y-4 w-[300px]">
      <div className="space-y-1.5">
        <Label>Default</Label>
        <Input placeholder="Default input" />
      </div>
      <div className="space-y-1.5">
        <Label>Focused</Label>
        <Input placeholder="Click to focus" autoFocus />
      </div>
      <div className="space-y-1.5">
        <Label>Disabled</Label>
        <Input placeholder="Disabled input" disabled />
      </div>
      <div className="space-y-1.5">
        <Label>Read Only</Label>
        <Input defaultValue="Read only value" readOnly />
      </div>
      <div className="space-y-1.5">
        <Label className="text-destructive">With Error</Label>
        <Input
          placeholder="Invalid input"
          className="border-destructive focus-visible:ring-destructive"
        />
        <p className="text-xs text-destructive">This field is required</p>
      </div>
    </div>
  ),
};

export const Sizes: Story = {
  name: 'Custom Sizes',
  render: () => (
    <div className="space-y-4 w-[300px]">
      <div className="space-y-1.5">
        <Label>Small</Label>
        <Input placeholder="Small input" className="h-7 text-xs" />
      </div>
      <div className="space-y-1.5">
        <Label>Default</Label>
        <Input placeholder="Default input" />
      </div>
      <div className="space-y-1.5">
        <Label>Large</Label>
        <Input placeholder="Large input" className="h-10 text-base" />
      </div>
    </div>
  ),
};

export const FileInput: Story = {
  name: 'File Input',
  render: () => (
    <div className="space-y-1.5 w-[300px]">
      <Label htmlFor="file">Upload File</Label>
      <Input id="file" type="file" />
    </div>
  ),
};

export const FormExample: Story = {
  name: 'Form Example',
  render: () => (
    <form className="space-y-4 w-[350px] p-4 border rounded-lg">
      <div className="space-y-1.5">
        <Label htmlFor="name">Full Name</Label>
        <Input id="name" placeholder="John Doe" />
      </div>
      <div className="space-y-1.5">
        <Label htmlFor="email2">Email Address</Label>
        <div className="relative">
          <Mail className="absolute left-2.5 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
          <Input
            id="email2"
            type="email"
            placeholder="john@example.com"
            className="pl-8"
          />
        </div>
      </div>
      <div className="space-y-1.5">
        <Label htmlFor="dob">Date of Birth</Label>
        <div className="relative">
          <Calendar className="absolute left-2.5 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
          <Input id="dob" type="date" className="pl-8" />
        </div>
      </div>
    </form>
  ),
};
