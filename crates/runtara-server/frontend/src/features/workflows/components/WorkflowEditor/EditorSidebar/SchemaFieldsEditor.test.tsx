import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';

import {
  SchemaFieldsEditor,
  applyAdvancedSchemaDraft,
  createAdvancedSchemaDraft,
} from './SchemaFieldsEditor';
import type { SchemaField } from './SchemaFieldsEditor';
import {
  buildSchemaFromFields,
  parseSchema,
} from '@/features/workflows/utils/schema';
import { validateSchemaFieldsWithRust } from '@/features/workflows/utils/rust-workflow-validation';

vi.mock('@/features/workflows/utils/rust-workflow-validation', () => ({
  validateSchemaFieldsWithRust: vi.fn(),
}));

const validValidationResult = {
  success: true,
  valid: true,
  status: 'valid' as const,
  errors: [],
  warnings: [],
  message: '',
  wasmAvailable: true,
  schemaErrors: [],
};

const baseField = (overrides: Partial<SchemaField> = {}): SchemaField => ({
  name: 'amount',
  type: 'string',
  required: true,
  description: '',
  ...overrides,
});

describe('SchemaFieldsEditor', () => {
  it('shows validation errors from the shared WASM validator', async () => {
    vi.mocked(validateSchemaFieldsWithRust).mockResolvedValue({
      success: true,
      valid: false,
      status: 'invalid',
      errors: [
        "[E008] Input schema field name 'order_id' is duplicated. Field names must be unique.",
      ],
      warnings: [],
      message: 'Schema field validation failed with 1 error(s)',
      wasmAvailable: true,
      schemaErrors: [
        {
          code: 'E008',
          message:
            "[E008] Input schema field name 'order_id' is duplicated. Field names must be unique.",
          fieldName: 'order_id',
          rowIndices: [0, 1],
        },
      ],
    });

    render(
      <SchemaFieldsEditor
        label="Input Schema Fields"
        fields={[
          {
            name: 'order_id',
            type: 'string',
            required: true,
            description: '',
          },
          {
            name: ' order_id ',
            type: 'number',
            required: false,
            description: '',
          },
        ]}
        onChange={vi.fn()}
      />
    );

    expect(
      await screen.findAllByText('Field name must be unique.')
    ).toHaveLength(2);
    const nameInputs = screen.getAllByPlaceholderText('fieldName');
    expect(nameInputs).toHaveLength(2);
    for (const input of nameInputs) {
      expect(input).toHaveAttribute('aria-invalid', 'true');
    }
  });
});

describe('advanced schema draft (dialog apply path)', () => {
  it('applies tier-1 display and validation inputs as typed values', () => {
    const field = baseField();
    const draft = createAdvancedSchemaDraft(field);

    const { field: applied, errors } = applyAdvancedSchemaDraft(field, {
      ...draft,
      label: 'Amount',
      placeholder: '0.00',
      order: '2',
      min: '1',
      max: '10',
      pattern: '^\\d+$',
      format: 'email',
      example: '"12"',
    });

    expect(errors).toEqual({});
    expect(applied).toEqual({
      name: 'amount',
      type: 'string',
      required: true,
      description: '',
      label: 'Amount',
      placeholder: '0.00',
      order: 2,
      min: 1,
      max: 10,
      pattern: '^\\d+$',
      format: 'email',
      example: '12',
    });
  });

  it('rejects non-integer order and invalid example JSON', () => {
    const field = baseField();
    const draft = createAdvancedSchemaDraft(field);

    const result = applyAdvancedSchemaDraft(field, {
      ...draft,
      order: '1.5',
      example: '{oops',
    });

    expect(result.field).toBeNull();
    expect(result.errors.order).toMatch(/integer/i);
    expect(result.errors.example).toMatch(/JSON/i);
  });

  it('builds scalar array items from the element-type select', () => {
    const field = baseField({ name: 'tags', type: 'array' });
    const draft = createAdvancedSchemaDraft(field);
    expect(draft.itemsType).toBe('string');

    const { field: applied, errors } = applyAdvancedSchemaDraft(field, {
      ...draft,
      itemsType: 'number',
    });

    expect(errors).toEqual({});
    expect(applied?.items).toEqual({ type: 'number' });
    expect(buildSchemaFromFields([applied!]).tags.items).toEqual({
      type: 'number',
    });
  });

  it('builds object array items from per-property fields', () => {
    const field = baseField({ name: 'lines', type: 'array' });
    const draft = createAdvancedSchemaDraft(field);

    const { field: applied, errors } = applyAdvancedSchemaDraft(field, {
      ...draft,
      itemsType: 'object',
      itemsProperties: [
        { name: 'sku', type: 'string', required: true, description: '' },
        {
          name: 'quantity',
          type: 'integer',
          required: false,
          description: '',
          min: 1,
        },
      ],
    });

    expect(errors).toEqual({});
    expect(applied?.items).toEqual({
      type: 'object',
      properties: {
        sku: { type: 'string', required: true },
        quantity: { type: 'integer', required: false, min: 1 },
      },
    });
  });

  it('preserves extra element keys when switching the items type', () => {
    const field = baseField({
      name: 'tags',
      type: 'array',
      items: { type: 'string', description: 'each tag' },
    });
    const draft = createAdvancedSchemaDraft(field);
    expect(draft.itemsType).toBe('string');

    const { field: applied } = applyAdvancedSchemaDraft(field, {
      ...draft,
      itemsType: 'integer',
    });

    expect(applied?.items).toEqual({ type: 'integer', description: 'each tag' });
  });

  it('round-trips an existing object element definition through the draft', () => {
    const items = {
      type: 'object',
      properties: {
        sku: { type: 'string', required: true },
        quantity: { type: 'integer', required: false, min: 1 },
      },
    };
    const field = baseField({ name: 'lines', type: 'array', items });

    const draft = createAdvancedSchemaDraft(field);
    expect(draft.itemsProperties.map((p) => p.name)).toEqual([
      'sku',
      'quantity',
    ]);

    const { field: applied, errors } = applyAdvancedSchemaDraft(field, draft);
    expect(errors).toEqual({});
    expect(applied?.items).toEqual(items);
  });

  it('keeps two levels of nested object properties intact', () => {
    const field = baseField({ name: 'customer', type: 'object' });
    const draft = createAdvancedSchemaDraft(field);
    const properties: SchemaField[] = [
      { name: 'name', type: 'string', required: true, description: '' },
      {
        name: 'address',
        type: 'object',
        required: false,
        description: '',
        properties: [
          { name: 'city', type: 'string', required: true, description: '' },
        ],
      },
    ];

    const { field: applied, errors } = applyAdvancedSchemaDraft(field, {
      ...draft,
      properties,
    });

    expect(errors).toEqual({});
    expect(applied?.properties).toEqual(properties);
    expect(buildSchemaFromFields([applied!])).toEqual({
      customer: {
        type: 'object',
        required: true,
        properties: {
          name: { type: 'string', required: true },
          address: {
            type: 'object',
            required: false,
            properties: {
              city: { type: 'string', required: true },
            },
          },
        },
      },
    });
  });

  it('builds visibleWhen from structured field/operator/value rows', () => {
    const field = baseField({ name: 'notes' });
    const draft = createAdvancedSchemaDraft(field);

    const single = applyAdvancedSchemaDraft(field, {
      ...draft,
      visibleWhenField: 'mode',
      visibleWhenRows: [{ operator: 'equals', value: 'manual' }],
    });
    expect(single.field?.visibleWhen).toEqual({
      field: 'mode',
      equals: 'manual',
    });

    const both = applyAdvancedSchemaDraft(field, {
      ...draft,
      visibleWhenField: 'approved',
      visibleWhenRows: [
        { operator: 'equals', value: 'false' },
        { operator: 'notEquals', value: '"false"' },
      ],
    });
    expect(both.field?.visibleWhen).toEqual({
      field: 'approved',
      equals: false,
      notEquals: 'false',
    });
  });

  it('loads an existing visibleWhen rule and requires a sibling field name', () => {
    const field = baseField({
      visibleWhen: { field: 'mode', equals: 'manual' },
    });
    const draft = createAdvancedSchemaDraft(field);

    expect(draft.visibleWhenField).toBe('mode');
    expect(draft.visibleWhenRows).toEqual([
      { operator: 'equals', value: 'manual' },
    ]);

    const roundTrip = applyAdvancedSchemaDraft(field, draft);
    expect(roundTrip.field?.visibleWhen).toEqual({
      field: 'mode',
      equals: 'manual',
    });

    const missing = applyAdvancedSchemaDraft(field, {
      ...draft,
      visibleWhenField: '  ',
    });
    expect(missing.field).toBeNull();
    expect(missing.errors.visibleWhen).toBeTruthy();
  });

  it('preserves unknown extensions alongside structured edits', () => {
    const field = baseField({
      extensions: { 'x-runtime': { source: 'fixture' } },
    });
    const draft = createAdvancedSchemaDraft(field);
    expect(JSON.parse(draft.extensionsText)).toEqual({
      'x-runtime': { source: 'fixture' },
    });

    const { field: applied, errors } = applyAdvancedSchemaDraft(field, {
      ...draft,
      label: 'Amount',
    });

    expect(errors).toEqual({});
    expect(applied?.extensions).toEqual({ 'x-runtime': { source: 'fixture' } });
    expect(applied?.label).toBe('Amount');
    expect(buildSchemaFromFields([applied!]).amount).toMatchObject({
      'x-runtime': { source: 'fixture' },
      label: 'Amount',
    });
  });

  it('rejects reserved keys in the extensions JSON instead of clobbering', () => {
    const field = baseField();
    const draft = createAdvancedSchemaDraft(field);

    const result = applyAdvancedSchemaDraft(field, {
      ...draft,
      label: 'Amount',
      extensionsText: '{"label": "Clobber", "x-ok": 1}',
    });

    expect(result.field).toBeNull();
    expect(result.errors.extensions).toContain('label');
  });

  it('round-trips a fully decorated field through an unchanged draft', () => {
    const rawSchema = {
      items: {
        type: 'array',
        required: true,
        description: 'Order line items',
        default: [],
        example: [{ sku: 'sku_1', quantity: 2 }],
        items: {
          type: 'object',
          properties: {
            sku: { type: 'string', required: true },
            quantity: { type: 'integer', required: false, min: 1 },
          },
        },
        enum: [['sku_1'], ['sku_2']],
        label: 'Items',
        placeholder: '[]',
        order: 2,
        format: 'json',
        min: 1,
        max: 50,
        pattern: '^.+$',
        visibleWhen: { field: 'mode', equals: 'manual' },
        'x-runtime': { source: 'fixture' },
      },
    };

    const [field] = parseSchema(rawSchema) as unknown as SchemaField[];
    const draft = createAdvancedSchemaDraft(field);
    const { field: applied, errors } = applyAdvancedSchemaDraft(field, draft);

    expect(errors).toEqual({});
    expect(buildSchemaFromFields([applied!])).toEqual(rawSchema);
  });
});

describe('AdvancedSchemaFieldDialog integration', () => {
  it('applies structured advanced edits through the dialog', async () => {
    vi.mocked(validateSchemaFieldsWithRust).mockResolvedValue(
      validValidationResult
    );
    const onChange = vi.fn();

    render(
      <SchemaFieldsEditor
        label="Input Schema Fields"
        fields={[baseField()]}
        onChange={onChange}
      />
    );

    fireEvent.click(
      screen.getByRole('button', { name: 'Edit advanced schema for amount' })
    );

    fireEvent.change(await screen.findByLabelText('Label'), {
      target: { value: 'Amount' },
    });
    fireEvent.change(screen.getByLabelText('Placeholder'), {
      target: { value: '0.00' },
    });
    fireEvent.change(screen.getByLabelText('Order'), {
      target: { value: '2' },
    });
    fireEvent.change(screen.getByLabelText('Min'), {
      target: { value: '1' },
    });
    fireEvent.change(screen.getByLabelText('Max'), {
      target: { value: '10' },
    });
    fireEvent.change(screen.getByLabelText('Pattern'), {
      target: { value: '^\\d+$' },
    });

    fireEvent.click(screen.getByRole('button', { name: 'Apply' }));

    expect(onChange).toHaveBeenCalledWith([
      expect.objectContaining({
        name: 'amount',
        label: 'Amount',
        placeholder: '0.00',
        order: 2,
        min: 1,
        max: 10,
        pattern: '^\\d+$',
      }),
    ]);
  });

  it('shows a live warning for invalid regex patterns', async () => {
    vi.mocked(validateSchemaFieldsWithRust).mockResolvedValue(
      validValidationResult
    );

    render(
      <SchemaFieldsEditor
        label="Input Schema Fields"
        fields={[baseField()]}
        onChange={vi.fn()}
      />
    );

    fireEvent.click(
      screen.getByRole('button', { name: 'Edit advanced schema for amount' })
    );
    fireEvent.change(await screen.findByLabelText('Pattern'), {
      target: { value: '(' },
    });

    expect(
      await screen.findByText('Not a valid regular expression.')
    ).toBeInTheDocument();
  });

  it('keeps unknown extensions behind a collapsed JSON section', async () => {
    vi.mocked(validateSchemaFieldsWithRust).mockResolvedValue(
      validValidationResult
    );

    render(
      <SchemaFieldsEditor
        label="Input Schema Fields"
        fields={[
          baseField({ extensions: { 'x-runtime': { source: 'fixture' } } }),
        ]}
        onChange={vi.fn()}
      />
    );

    fireEvent.click(
      screen.getByRole('button', { name: 'Edit advanced schema for amount' })
    );

    expect(
      await screen.findByText('Unknown extensions (JSON) (1)')
    ).toBeInTheDocument();
    const textarea = screen.getByLabelText(
      'Unknown extensions (JSON)'
    ) as HTMLTextAreaElement;
    expect(textarea.value).toContain('x-runtime');
  });
});
