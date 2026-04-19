import type { Meta, StoryObj } from '@storybook/react';
import { fn } from '@storybook/test';
import { useState } from 'react';
import {
  ConditionEditor,
  Condition,
  renderConditionReadable,
} from './condition-editor';

const meta: Meta<typeof ConditionEditor> = {
  title: 'Forms/ConditionEditor',
  component: ConditionEditor,
  parameters: {
    layout: 'padded',
    docs: {
      description: {
        component:
          'A visual boolean logic builder supporting nested conditions with 15+ operators. Used for building conditional expressions in workflow workflows.',
      },
    },
  },
  tags: ['autodocs'],
  argTypes: {
    value: {
      control: 'text',
      description: 'JSON string representation of the condition',
    },
    disabled: {
      control: 'boolean',
      description: 'Disable the editor',
    },
    previousSteps: {
      control: 'object',
      description: 'Array of previous workflow steps for variable autocomplete',
    },
  },
  args: {
    onChange: fn(),
  },
};

export default meta;
type Story = StoryObj<typeof ConditionEditor>;

// Helper to create condition JSON
const createCondition = (condition: Condition): string =>
  JSON.stringify(condition);

// Sample previous steps for autocomplete
const samplePreviousSteps = [
  {
    id: 'step-1',
    name: 'Fetch User Data',
    outputs: [
      { path: "steps['step-1'].outputs.user", type: 'object', name: 'user' },
      {
        path: "steps['step-1'].outputs.user.name",
        type: 'string',
        name: 'name',
      },
      { path: "steps['step-1'].outputs.user.age", type: 'number', name: 'age' },
      {
        path: "steps['step-1'].outputs.user.isActive",
        type: 'boolean',
        name: 'isActive',
      },
    ],
  },
  {
    id: 'step-2',
    name: 'Calculate Total',
    outputs: [
      { path: "steps['step-2'].outputs.total", type: 'number', name: 'total' },
      { path: "steps['step-2'].outputs.items", type: 'array', name: 'items' },
    ],
  },
];

export const Default: Story = {
  args: {
    value: createCondition({
      type: 'operation',
      op: 'EQ',
      arguments: [
        { valueType: 'immediate', value: '', immediateType: 'string' },
        { valueType: 'immediate', value: '', immediateType: 'string' },
      ],
    }),
  },
};

export const SimpleComparison: Story = {
  name: 'Simple Comparison (EQ)',
  args: {
    value: createCondition({
      type: 'operation',
      op: 'EQ',
      arguments: [
        { valueType: 'immediate', value: 'status', immediateType: 'string' },
        { valueType: 'immediate', value: 'active', immediateType: 'string' },
      ],
    }),
  },
};

export const NumericComparison: Story = {
  name: 'Numeric Comparison (GT)',
  args: {
    value: createCondition({
      type: 'operation',
      op: 'GT',
      arguments: [
        { valueType: 'immediate', value: '100', immediateType: 'number' },
        { valueType: 'immediate', value: '50', immediateType: 'number' },
      ],
    }),
  },
};

export const BooleanCheck: Story = {
  name: 'Boolean Check (IS_DEFINED)',
  args: {
    value: createCondition({
      type: 'operation',
      op: 'IS_DEFINED',
      arguments: [
        {
          valueType: 'immediate',
          value: 'user.email',
          immediateType: 'string',
        },
      ],
    }),
  },
};

export const LogicalAnd: Story = {
  name: 'Logical AND (Multiple Conditions)',
  args: {
    value: createCondition({
      type: 'operation',
      op: 'AND',
      arguments: [
        {
          type: 'operation',
          op: 'GT',
          arguments: [
            { valueType: 'immediate', value: 'age', immediateType: 'string' },
            { valueType: 'immediate', value: '18', immediateType: 'number' },
          ],
        },
        {
          type: 'operation',
          op: 'EQ',
          arguments: [
            {
              valueType: 'immediate',
              value: 'status',
              immediateType: 'string',
            },
            {
              valueType: 'immediate',
              value: 'active',
              immediateType: 'string',
            },
          ],
        },
      ],
    }),
  },
};

export const LogicalOr: Story = {
  name: 'Logical OR',
  args: {
    value: createCondition({
      type: 'operation',
      op: 'OR',
      arguments: [
        {
          type: 'operation',
          op: 'EQ',
          arguments: [
            { valueType: 'immediate', value: 'role', immediateType: 'string' },
            { valueType: 'immediate', value: 'admin', immediateType: 'string' },
          ],
        },
        {
          type: 'operation',
          op: 'EQ',
          arguments: [
            { valueType: 'immediate', value: 'role', immediateType: 'string' },
            {
              valueType: 'immediate',
              value: 'superuser',
              immediateType: 'string',
            },
          ],
        },
      ],
    }),
  },
};

export const WithReferences: Story = {
  name: 'With Variable References',
  args: {
    value: createCondition({
      type: 'operation',
      op: 'EQ',
      arguments: [
        {
          valueType: 'reference',
          value: "steps['step-1'].outputs.user.isActive",
        },
        { valueType: 'immediate', value: 'true', immediateType: 'boolean' },
      ],
    }),
    previousSteps: samplePreviousSteps,
  },
};

export const NestedConditions: Story = {
  name: 'Deeply Nested Conditions',
  args: {
    value: createCondition({
      type: 'operation',
      op: 'AND',
      arguments: [
        {
          type: 'operation',
          op: 'OR',
          arguments: [
            {
              type: 'operation',
              op: 'EQ',
              arguments: [
                {
                  valueType: 'immediate',
                  value: 'type',
                  immediateType: 'string',
                },
                {
                  valueType: 'immediate',
                  value: 'premium',
                  immediateType: 'string',
                },
              ],
            },
            {
              type: 'operation',
              op: 'GT',
              arguments: [
                {
                  valueType: 'immediate',
                  value: 'credits',
                  immediateType: 'string',
                },
                {
                  valueType: 'immediate',
                  value: '1000',
                  immediateType: 'number',
                },
              ],
            },
          ],
        },
        {
          type: 'operation',
          op: 'IS_NOT_EMPTY',
          arguments: [
            { valueType: 'immediate', value: 'email', immediateType: 'string' },
          ],
        },
      ],
    }),
  },
};

export const InListOperator: Story = {
  name: 'IN List Operator',
  args: {
    value: createCondition({
      type: 'operation',
      op: 'IN',
      arguments: [
        { valueType: 'immediate', value: 'status', immediateType: 'string' },
        {
          valueType: 'immediate',
          value: '["active", "pending", "approved"]',
          immediateType: 'string',
        },
      ],
    }),
  },
};

export const ContainsOperator: Story = {
  name: 'CONTAINS Operator',
  args: {
    value: createCondition({
      type: 'operation',
      op: 'CONTAINS',
      arguments: [
        {
          valueType: 'immediate',
          value: 'description',
          immediateType: 'string',
        },
        { valueType: 'immediate', value: 'error', immediateType: 'string' },
      ],
    }),
  },
};

export const Disabled: Story = {
  args: {
    value: createCondition({
      type: 'operation',
      op: 'EQ',
      arguments: [
        { valueType: 'immediate', value: 'status', immediateType: 'string' },
        { valueType: 'immediate', value: 'active', immediateType: 'string' },
      ],
    }),
    disabled: true,
  },
};

export const WithPreviousSteps: Story = {
  name: 'With Autocomplete (Previous Steps)',
  args: {
    value: createCondition({
      type: 'operation',
      op: 'GT',
      arguments: [
        { valueType: 'reference', value: "steps['step-2'].outputs.total" },
        { valueType: 'immediate', value: '100', immediateType: 'number' },
      ],
    }),
    previousSteps: samplePreviousSteps,
  },
};

// Interactive example with state
const InteractiveExample = () => {
  const [value, setValue] = useState<string>(
    createCondition({
      type: 'operation',
      op: 'EQ',
      arguments: [
        { valueType: 'immediate', value: '', immediateType: 'string' },
        { valueType: 'immediate', value: '', immediateType: 'string' },
      ],
    })
  );

  const parsedCondition = value ? (JSON.parse(value) as Condition) : undefined;
  const readable = parsedCondition
    ? renderConditionReadable(parsedCondition)
    : '';

  return (
    <div className="space-y-4">
      <ConditionEditor
        value={value}
        onChange={setValue}
        previousSteps={samplePreviousSteps}
      />
      <div className="p-3 bg-slate-100 dark:bg-slate-800 rounded-lg">
        <p className="text-xs font-medium text-slate-500 dark:text-slate-400 mb-1">
          Readable Expression:
        </p>
        <p className="text-sm font-mono">{readable || '(empty)'}</p>
      </div>
      <div className="p-3 bg-slate-100 dark:bg-slate-800 rounded-lg">
        <p className="text-xs font-medium text-slate-500 dark:text-slate-400 mb-1">
          JSON Value:
        </p>
        <pre className="text-xs font-mono overflow-auto max-h-40">
          {JSON.stringify(parsedCondition, null, 2)}
        </pre>
      </div>
    </div>
  );
};

export const Interactive: Story = {
  render: () => <InteractiveExample />,
  parameters: {
    docs: {
      description: {
        story:
          'Try building different conditions interactively. Shows the readable expression and JSON output.',
      },
    },
  },
};

// All operators showcase
export const AllOperators: Story = {
  name: 'All Operators Reference',
  render: () => (
    <div className="space-y-6">
      <div className="grid grid-cols-2 gap-4">
        <div className="p-3 bg-slate-50 dark:bg-slate-900 rounded-lg">
          <h4 className="text-xs font-semibold text-slate-500 mb-2">
            Logic Operators
          </h4>
          <ul className="text-sm space-y-1">
            <li>
              <code className="text-xs">AND</code> - Logical AND (variadic)
            </li>
            <li>
              <code className="text-xs">OR</code> - Logical OR (variadic)
            </li>
            <li>
              <code className="text-xs">NOT</code> - Logical NOT (unary)
            </li>
          </ul>
        </div>
        <div className="p-3 bg-slate-50 dark:bg-slate-900 rounded-lg">
          <h4 className="text-xs font-semibold text-slate-500 mb-2">
            Comparison Operators
          </h4>
          <ul className="text-sm space-y-1">
            <li>
              <code className="text-xs">EQ</code> - Equals
            </li>
            <li>
              <code className="text-xs">NE</code> - Not Equals
            </li>
            <li>
              <code className="text-xs">GT/GTE</code> - Greater Than (or Equal)
            </li>
            <li>
              <code className="text-xs">LT/LTE</code> - Less Than (or Equal)
            </li>
          </ul>
        </div>
        <div className="p-3 bg-slate-50 dark:bg-slate-900 rounded-lg">
          <h4 className="text-xs font-semibold text-slate-500 mb-2">
            Check Operators
          </h4>
          <ul className="text-sm space-y-1">
            <li>
              <code className="text-xs">IS_EMPTY</code> - Check if empty
            </li>
            <li>
              <code className="text-xs">IS_NOT_EMPTY</code> - Check if not empty
            </li>
            <li>
              <code className="text-xs">IS_DEFINED</code> - Check if defined
            </li>
            <li>
              <code className="text-xs">LENGTH</code> - Get length
            </li>
          </ul>
        </div>
        <div className="p-3 bg-slate-50 dark:bg-slate-900 rounded-lg">
          <h4 className="text-xs font-semibold text-slate-500 mb-2">
            List/String Operators
          </h4>
          <ul className="text-sm space-y-1">
            <li>
              <code className="text-xs">IN</code> - Value in list
            </li>
            <li>
              <code className="text-xs">NOT_IN</code> - Value not in list
            </li>
            <li>
              <code className="text-xs">CONTAINS</code> - Contains substring
            </li>
          </ul>
        </div>
      </div>
    </div>
  ),
};
