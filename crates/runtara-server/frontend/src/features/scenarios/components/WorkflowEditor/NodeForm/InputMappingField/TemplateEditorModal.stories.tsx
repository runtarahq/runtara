import type { Meta, StoryObj } from '@storybook/react';
import { useState } from 'react';
import { fn } from '@storybook/test';
import { TemplateEditorModal } from './TemplateEditorModal';
import { Button } from '@/shared/components/ui/button';
import { Code } from 'lucide-react';

// Note: This component requires NodeFormContext and nodeFormStore for full functionality.
// For Storybook, we create a simplified version that demonstrates the UI.

const meta: Meta<typeof TemplateEditorModal> = {
  title: 'Scenarios/TemplateEditorModal',
  component: TemplateEditorModal,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'A modal dialog for editing Jinja2-style templates with syntax highlighting, variable browser, and live preview. Features editor/preview/split view modes.',
      },
    },
  },
  tags: ['autodocs'],
  argTypes: {
    open: {
      control: 'boolean',
      description: 'Whether the modal is open',
    },
    value: {
      control: 'text',
      description: 'Template content',
    },
    fieldName: {
      control: 'text',
      description: 'Name of the field being edited (shown in header)',
    },
    placeholder: {
      control: 'text',
      description: 'Placeholder text for empty editor',
    },
  },
  args: {
    onOpenChange: fn(),
    onChange: fn(),
  },
};

export default meta;
type Story = StoryObj<typeof TemplateEditorModal>;

// Sample templates
const simpleTemplate = `Hello {{ name }}!

Welcome to our service.`;

const conditionalTemplate = `Dear {{ customer_name }},

{% if order_total > 100 %}
Thank you for your large order!
{% else %}
Thank you for your order.
{% endif %}

Best regards,
{{ company_name }}`;

const loopTemplate = [
  'Order Summary:',
  '',
  '{% for item in items %}',
  '- {{ item.name }}: ${{ item.price }}',
  '{% endfor %}',
  '',
  'Total: ${{ total | default("0.00") }}',
].join('\n');

const complexTemplate = [
  '{# Email template for order confirmation #}',
  'Subject: Order Confirmation - {{ order_id }}',
  '',
  'Dear {{ customer.name | default("Valued Customer") }},',
  '',
  'Thank you for your order!',
  '',
  'Order Details:',
  '--------------',
  'Order ID: {{ order_id }}',
  'Date: {{ order_date }}',
  '',
  'Items:',
  '{% for item in items %}',
  '  {{ item.quantity }}x {{ item.name }} @ ${{ item.price }}',
  '{% endfor %}',
  '',
  '{% if discount_applied %}',
  'Discount: -${{ discount_amount }}',
  '{% endif %}',
  '',
  'Subtotal: ${{ subtotal }}',
  'Tax: ${{ tax }}',
  '--------------',
  'Total: ${{ total }}',
  '',
  '{% if notes %}',
  'Special Instructions: {{ notes }}',
  '{% endif %}',
  '',
  'Thank you for shopping with us!',
  '',
  'Best regards,',
  '{{ company_name }}',
].join('\n');

export const Default: Story = {
  args: {
    open: true,
    value: '',
    fieldName: 'Template Field',
  },
};

export const WithContent: Story = {
  name: 'With Content',
  args: {
    open: true,
    value: simpleTemplate,
    fieldName: 'Welcome Message',
  },
};

export const ConditionalTemplate: Story = {
  name: 'Conditional Template',
  args: {
    open: true,
    value: conditionalTemplate,
    fieldName: 'Order Email',
  },
};

export const LoopTemplate: Story = {
  name: 'Loop Template',
  args: {
    open: true,
    value: loopTemplate,
    fieldName: 'Order Summary',
  },
};

export const ComplexTemplate: Story = {
  name: 'Complex Template',
  args: {
    open: true,
    value: complexTemplate,
    fieldName: 'Order Confirmation Email',
  },
};

// Interactive example with state
const InteractiveExample = () => {
  const [open, setOpen] = useState(false);
  const [template, setTemplate] = useState(simpleTemplate);

  return (
    <div className="space-y-4">
      <Button onClick={() => setOpen(true)}>
        <Code className="w-4 h-4 mr-2" />
        Open Template Editor
      </Button>
      <div className="p-4 bg-muted rounded-lg max-w-md">
        <p className="text-sm font-medium mb-2">Current Template:</p>
        <pre className="text-xs font-mono whitespace-pre-wrap text-muted-foreground">
          {template || '(empty)'}
        </pre>
      </div>
      <TemplateEditorModal
        open={open}
        onOpenChange={setOpen}
        value={template}
        onChange={setTemplate}
        fieldName="My Template"
      />
    </div>
  );
};

export const Interactive: Story = {
  render: () => <InteractiveExample />,
  parameters: {
    docs: {
      description: {
        story:
          'Click the button to open the template editor. Changes are saved when you click "Save Template".',
      },
    },
  },
};

// Example showing different starting templates
const TemplateExamples = () => {
  const [open, setOpen] = useState(false);
  const [currentTemplate, setCurrentTemplate] = useState('');
  const [selectedName, setSelectedName] = useState('');

  const templates = [
    { name: 'Simple Variable', value: 'Hello {{ name }}!' },
    { name: 'Conditional', value: conditionalTemplate },
    { name: 'Loop', value: loopTemplate },
    { name: 'Complex', value: complexTemplate },
  ];

  const openWithTemplate = (name: string, value: string) => {
    setSelectedName(name);
    setCurrentTemplate(value);
    setOpen(true);
  };

  return (
    <div className="space-y-4">
      <div className="grid grid-cols-2 gap-2">
        {templates.map((t) => (
          <Button
            key={t.name}
            variant="outline"
            onClick={() => openWithTemplate(t.name, t.value)}
          >
            {t.name}
          </Button>
        ))}
      </div>
      <TemplateEditorModal
        open={open}
        onOpenChange={setOpen}
        value={currentTemplate}
        onChange={setCurrentTemplate}
        fieldName={selectedName}
      />
    </div>
  );
};

export const TemplateGallery: Story = {
  name: 'Template Gallery',
  render: () => <TemplateExamples />,
  parameters: {
    docs: {
      description: {
        story:
          'Click different buttons to open the editor with various template examples.',
      },
    },
  },
};

export const FeatureReference: Story = {
  name: 'Feature Reference',
  render: () => (
    <div className="max-w-lg space-y-4 p-4 bg-background border rounded-lg">
      <h3 className="font-semibold">Template Editor Features</h3>

      <div className="space-y-3 text-sm">
        <div>
          <h4 className="font-medium text-primary">Jinja2 Syntax Support</h4>
          <ul className="text-muted-foreground text-xs space-y-1 mt-1">
            <li>
              <code className="bg-blue-100 dark:bg-blue-900/50 px-1 rounded">
                {'{{ variable }}'}
              </code>{' '}
              - Variable interpolation
            </li>
            <li>
              <code className="bg-purple-100 dark:bg-purple-900/50 px-1 rounded">
                {'{% if/for %}'}
              </code>{' '}
              - Control structures
            </li>
            <li>
              <code className="bg-muted px-1 rounded">{'{# comment #}'}</code> -
              Comments
            </li>
          </ul>
        </div>

        <div>
          <h4 className="font-medium text-primary">View Modes</h4>
          <ul className="text-muted-foreground text-xs space-y-1 mt-1">
            <li>
              <strong>Editor</strong> - Full template editing
            </li>
            <li>
              <strong>Preview</strong> - See rendered output with sample data
            </li>
            <li>
              <strong>Split</strong> - Side-by-side editor and preview
            </li>
          </ul>
        </div>

        <div>
          <h4 className="font-medium text-primary">Quick Insert Snippets</h4>
          <ul className="text-muted-foreground text-xs space-y-1 mt-1">
            <li>
              <code className="px-1 bg-muted rounded">if</code> - Conditional
              block
            </li>
            <li>
              <code className="px-1 bg-muted rounded">for</code> - Loop block
            </li>
            <li>
              <code className="px-1 bg-muted rounded">default</code> - Default
              value filter
            </li>
            <li>
              <code className="px-1 bg-muted rounded">comment</code> - Comment
              block
            </li>
          </ul>
        </div>

        <div>
          <h4 className="font-medium text-primary">Variables Panel</h4>
          <p className="text-muted-foreground text-xs mt-1">
            Shows available template variables from the context field. Click to
            insert at cursor position.
          </p>
        </div>
      </div>
    </div>
  ),
};
