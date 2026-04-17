import type { Meta, StoryObj } from '@storybook/react';
import { useState } from 'react';
import { FilterComposer } from './index';
import { Condition } from '@/generated/RuntaraRuntimeApi';

const meta: Meta<typeof FilterComposer> = {
  title: 'Objects/FilterComposer',
  component: FilterComposer,
  parameters: {
    layout: 'padded',
    docs: {
      description: {
        component:
          'A visual condition builder for creating filter expressions. Supports multiple operators (AND, OR, NOT, comparison operators) and nested conditions.',
      },
    },
  },
  tags: ['autodocs'],
};

export default meta;
type Story = StoryObj<typeof FilterComposer>;

// Sample schema definitions
const orderSchema = {
  orderId: { name: 'Order ID', dataType: 'STRING', required: true },
  customerEmail: { name: 'Customer Email', dataType: 'STRING' },
  totalAmount: { name: 'Total Amount', dataType: 'DECIMAL' },
  status: { name: 'Status', dataType: 'STRING' },
  createdAt: { name: 'Created At', dataType: 'DATE' },
  itemCount: { name: 'Item Count', dataType: 'INTEGER' },
  isPaid: { name: 'Is Paid', dataType: 'BOOLEAN' },
};

const productSchema = {
  sku: { name: 'SKU', dataType: 'STRING', required: true },
  name: { name: 'Product Name', dataType: 'STRING' },
  price: { name: 'Price', dataType: 'DECIMAL' },
  quantity: { name: 'Stock Quantity', dataType: 'INTEGER' },
  category: { name: 'Category', dataType: 'STRING' },
  isActive: { name: 'Is Active', dataType: 'BOOLEAN' },
};

export const Default: Story = {
  args: {
    value: null,
  },
};

export const WithSchema: Story = {
  name: 'With Schema Definition',
  args: {
    value: null,
    schemaDefinition: orderSchema,
  },
  parameters: {
    docs: {
      description: {
        story:
          'When a schema is provided, the first argument shows a dropdown with field names.',
      },
    },
  },
};

export const SimpleCondition: Story = {
  name: 'Simple Condition',
  args: {
    value: {
      op: 'EQ',
      arguments: ['status', 'active'],
    },
    schemaDefinition: orderSchema,
  },
};

export const ComparisonCondition: Story = {
  name: 'Comparison Condition',
  args: {
    value: {
      op: 'GT',
      arguments: ['totalAmount', '100'],
    },
    schemaDefinition: orderSchema,
  },
};

export const AndCondition: Story = {
  name: 'AND Condition',
  args: {
    value: {
      op: 'AND',
      arguments: [
        { op: 'EQ', arguments: ['status', 'active'] },
        { op: 'GT', arguments: ['totalAmount', '50'] },
      ],
    },
    schemaDefinition: orderSchema,
  },
};

export const OrCondition: Story = {
  name: 'OR Condition',
  args: {
    value: {
      op: 'OR',
      arguments: [
        { op: 'EQ', arguments: ['status', 'pending'] },
        { op: 'EQ', arguments: ['status', 'processing'] },
      ],
    },
    schemaDefinition: orderSchema,
  },
};

export const NotCondition: Story = {
  name: 'NOT Condition',
  args: {
    value: {
      op: 'NOT',
      arguments: [{ op: 'EQ', arguments: ['status', 'cancelled'] }],
    },
    schemaDefinition: orderSchema,
  },
};

export const UnaryOperators: Story = {
  name: 'Unary Operators',
  render: () => (
    <div className="space-y-6">
      <div>
        <p className="text-sm font-medium mb-2">IS_EMPTY</p>
        <FilterComposer
          value={{ op: 'IS_EMPTY', arguments: ['customerEmail'] }}
          schemaDefinition={orderSchema}
        />
      </div>
      <div>
        <p className="text-sm font-medium mb-2">IS_NOT_EMPTY</p>
        <FilterComposer
          value={{ op: 'IS_NOT_EMPTY', arguments: ['customerEmail'] }}
          schemaDefinition={orderSchema}
        />
      </div>
      <div>
        <p className="text-sm font-medium mb-2">IS_DEFINED</p>
        <FilterComposer
          value={{ op: 'IS_DEFINED', arguments: ['totalAmount'] }}
          schemaDefinition={orderSchema}
        />
      </div>
    </div>
  ),
};

export const ComplexNested: Story = {
  name: 'Complex Nested Condition',
  args: {
    value: {
      op: 'AND',
      arguments: [
        {
          op: 'OR',
          arguments: [
            { op: 'EQ', arguments: ['status', 'shipped'] },
            { op: 'EQ', arguments: ['status', 'delivered'] },
          ],
        },
        { op: 'GT', arguments: ['totalAmount', '100'] },
        { op: 'IS_NOT_EMPTY', arguments: ['customerEmail'] },
      ],
    },
    schemaDefinition: orderSchema,
  },
};

export const InListCondition: Story = {
  name: 'IN List Condition',
  args: {
    value: {
      op: 'IN',
      arguments: ['status', 'pending,processing,shipped'],
    },
    schemaDefinition: orderSchema,
  },
};

export const ContainsCondition: Story = {
  name: 'CONTAINS Condition',
  args: {
    value: {
      op: 'CONTAINS',
      arguments: ['customerEmail', '@example.com'],
    },
    schemaDefinition: orderSchema,
  },
};

// Interactive example with state
const InteractiveExample = () => {
  const [condition, setCondition] = useState<Condition | null>(null);

  return (
    <div className="space-y-4">
      <FilterComposer
        value={condition}
        onChange={setCondition}
        schemaDefinition={orderSchema}
      />
      <div className="p-3 bg-muted rounded text-sm">
        <strong>Current Value:</strong>
        <pre className="mt-2 text-xs overflow-auto">
          {JSON.stringify(condition, null, 2) || 'null'}
        </pre>
      </div>
    </div>
  );
};

export const Interactive: Story = {
  name: 'Interactive',
  render: () => <InteractiveExample />,
  parameters: {
    docs: {
      description: {
        story:
          'Build your own filter condition interactively. The JSON output is shown below.',
      },
    },
  },
};

// Different schema example
export const ProductSchema: Story = {
  name: 'Product Schema',
  args: {
    value: {
      op: 'AND',
      arguments: [
        { op: 'EQ', arguments: ['isActive', 'true'] },
        { op: 'GT', arguments: ['quantity', '0'] },
      ],
    },
    schemaDefinition: productSchema,
  },
};

export const AllOperators: Story = {
  name: 'All Operators Reference',
  render: () => (
    <div className="space-y-4 text-sm">
      <div className="grid grid-cols-3 gap-4 p-4 bg-muted rounded-lg">
        <div>
          <h4 className="font-semibold mb-2">Variadic</h4>
          <ul className="space-y-1 text-muted-foreground text-xs">
            <li>AND - All must be true</li>
            <li>OR - Any must be true</li>
          </ul>
        </div>
        <div>
          <h4 className="font-semibold mb-2">Binary</h4>
          <ul className="space-y-1 text-muted-foreground text-xs">
            <li>EQ - Equals</li>
            <li>NE - Not Equals</li>
            <li>GT - Greater Than</li>
            <li>GTE - Greater or Equal</li>
            <li>LT - Less Than</li>
            <li>LTE - Less or Equal</li>
            <li>IN - In List</li>
            <li>NOT_IN - Not In List</li>
            <li>CONTAINS - Contains</li>
          </ul>
        </div>
        <div>
          <h4 className="font-semibold mb-2">Unary</h4>
          <ul className="space-y-1 text-muted-foreground text-xs">
            <li>NOT - Invert</li>
            <li>IS_EMPTY - Is Empty</li>
            <li>IS_NOT_EMPTY - Not Empty</li>
            <li>IS_DEFINED - Is Defined</li>
          </ul>
        </div>
      </div>
      <FilterComposer value={null} schemaDefinition={orderSchema} />
    </div>
  ),
};
