import { describe, it, expect } from 'vitest';
import {
  inferOperandType,
  convertOperandValue,
  convertConditionArguments,
} from './condition-type-conversion';

describe('condition-type-conversion', () => {
  describe('inferOperandType', () => {
    it('should infer number type for GT operator second argument', () => {
      expect(inferOperandType('GT', 1, '200')).toBe('number');
    });

    it('should infer number type for GTE operator second argument', () => {
      expect(inferOperandType('GTE', 1, '100')).toBe('number');
    });

    it('should infer number type for LT operator second argument', () => {
      expect(inferOperandType('LT', 1, '50')).toBe('number');
    });

    it('should infer number type for LTE operator second argument', () => {
      expect(inferOperandType('LTE', 1, '75')).toBe('number');
    });

    it('should infer number type for nested LENGTH function', () => {
      const nestedCondition = { op: 'LENGTH', arguments: ['{{field}}'] };
      expect(inferOperandType('GT', 0, nestedCondition)).toBe('number');
    });

    it('should infer string type for EQ operator arguments without schema', () => {
      expect(inferOperandType('EQ', 0, 'field')).toBe('string');
      expect(inferOperandType('EQ', 1, 'value')).toBe('string');
    });

    it('should infer number type for EQ operator value when field is INTEGER', () => {
      expect(inferOperandType('EQ', 1, '12000', 'INTEGER')).toBe('number');
    });

    it('should infer number type for NE operator value when field is DECIMAL', () => {
      expect(inferOperandType('NE', 1, '3.14', 'DECIMAL')).toBe('number');
    });

    it('should infer boolean type for EQ operator value when field is BOOLEAN', () => {
      expect(inferOperandType('EQ', 1, 'true', 'BOOLEAN')).toBe('boolean');
    });

    it('should not use field data type for the first argument (field name)', () => {
      expect(inferOperandType('EQ', 0, 'price', 'INTEGER')).toBe('string');
    });

    it('should infer string type for first argument of GT operator', () => {
      expect(inferOperandType('GT', 0, 'field')).toBe('string');
    });

    it('should infer boolean type for nested IS_EMPTY function', () => {
      const nestedCondition = { op: 'IS_EMPTY', arguments: ['{{field}}'] };
      expect(inferOperandType('AND', 0, nestedCondition)).toBe('boolean');
    });
  });

  describe('convertOperandValue', () => {
    it('should convert valid numeric strings to numbers', () => {
      expect(convertOperandValue('200', 'number')).toBe(200);
      expect(convertOperandValue('3.14', 'number')).toBe(3.14);
      expect(convertOperandValue('-42', 'number')).toBe(-42);
      expect(convertOperandValue('0', 'number')).toBe(0);
    });

    it('should keep invalid numeric strings as strings', () => {
      expect(convertOperandValue('abc', 'number')).toBe('abc');
      expect(convertOperandValue('{{variable}}', 'number')).toBe(
        '{{variable}}'
      );
      expect(convertOperandValue('', 'number')).toBe('');
    });

    it('should convert boolean strings correctly', () => {
      expect(convertOperandValue('true', 'boolean')).toBe(true);
      expect(convertOperandValue('false', 'boolean')).toBe(false);
      expect(convertOperandValue('TRUE', 'boolean')).toBe(true);
      expect(convertOperandValue('FALSE', 'boolean')).toBe(false);
    });

    it('should keep non-boolean strings as strings', () => {
      expect(convertOperandValue('yes', 'boolean')).toBe('yes');
      expect(convertOperandValue('{{condition}}', 'boolean')).toBe(
        '{{condition}}'
      );
    });

    it('should preserve null and undefined', () => {
      expect(convertOperandValue(null, 'number')).toBe(null);
      expect(convertOperandValue(undefined, 'number')).toBe(undefined);
    });

    it('should preserve objects (nested conditions)', () => {
      const obj = { op: 'LENGTH', arguments: ['test'] };
      expect(convertOperandValue(obj, 'number')).toBe(obj);
    });
  });

  describe('convertConditionArguments', () => {
    it('should convert numeric comparison operands correctly', () => {
      const args = ['{{field}}', '200'];
      const converted = convertConditionArguments('GT', args);
      expect(converted).toEqual(['{{field}}', 200]);
    });

    it('should handle LENGTH > 200 case', () => {
      const args = [
        { op: 'LENGTH', arguments: ['{{data.node.descriptionHtml}}'] },
        '200',
      ];
      const converted = convertConditionArguments('GT', args);

      expect(converted).toEqual([
        { op: 'LENGTH', arguments: ['{{data.node.descriptionHtml}}'] },
        200, // Should be a number, not a string
      ]);
    });

    it('should handle nested conditions recursively', () => {
      const args = [
        { op: 'LENGTH', arguments: ['{{field}}'] },
        { op: 'COUNT', arguments: ['{{items}}'] },
      ];
      const converted = convertConditionArguments('GT', args);

      expect(converted).toEqual([
        { op: 'LENGTH', arguments: ['{{field}}'] },
        { op: 'COUNT', arguments: ['{{items}}'] },
      ]);
    });

    it('should preserve string operands for EQ operator without schema', () => {
      const args = ['name', 'John'];
      const converted = convertConditionArguments('EQ', args);
      expect(converted).toEqual(['name', 'John']);
    });

    it('should convert EQ value to number when schema says INTEGER', () => {
      const args = ['product_price', '12000'];
      const schema = { product_price: { dataType: 'INTEGER' } };
      const converted = convertConditionArguments('EQ', args, schema);
      expect(converted).toEqual(['product_price', 12000]);
    });

    it('should convert NE value to number when schema says DECIMAL', () => {
      const args = ['amount', '99.99'];
      const schema = { amount: { dataType: 'DECIMAL' } };
      const converted = convertConditionArguments('NE', args, schema);
      expect(converted).toEqual(['amount', 99.99]);
    });

    it('should convert EQ value to boolean when schema says BOOLEAN', () => {
      const args = ['is_active', 'true'];
      const schema = { is_active: { dataType: 'BOOLEAN' } };
      const converted = convertConditionArguments('EQ', args, schema);
      expect(converted).toEqual(['is_active', true]);
    });

    it('should keep string for EQ when schema says STRING', () => {
      const args = ['name', 'John'];
      const schema = { name: { dataType: 'STRING' } };
      const converted = convertConditionArguments('EQ', args, schema);
      expect(converted).toEqual(['name', 'John']);
    });

    it('should keep string for EQ when field is not in schema', () => {
      const args = ['unknown_field', '123'];
      const schema = { name: { dataType: 'STRING' } };
      const converted = convertConditionArguments('EQ', args, schema);
      expect(converted).toEqual(['unknown_field', '123']);
    });

    it('should pass schema through to nested conditions', () => {
      const args = [
        {
          op: 'EQ',
          arguments: ['product_price', '500'],
        },
        {
          op: 'EQ',
          arguments: ['name', 'Test'],
        },
      ];
      const schema = {
        product_price: { dataType: 'INTEGER' },
        name: { dataType: 'STRING' },
      };
      const converted = convertConditionArguments('AND', args, schema);
      expect(converted).toEqual([
        { op: 'EQ', arguments: ['product_price', 500] },
        { op: 'EQ', arguments: ['name', 'Test'] },
      ]);
    });

    it('should handle empty strings', () => {
      const args = ['field', ''];
      const converted = convertConditionArguments('GT', args);
      expect(converted).toEqual(['field', '']);
    });

    it('should handle template variables in numeric context', () => {
      const args = ['{{value1}}', '{{value2}}'];
      const converted = convertConditionArguments('GT', args);
      // Template variables should remain as strings (not convertible to numbers)
      expect(converted).toEqual(['{{value1}}', '{{value2}}']);
    });

    it('should handle complex nested case', () => {
      const args = [
        {
          op: 'LENGTH',
          arguments: [{ op: 'CONCAT', arguments: ['Hello', 'World'] }],
        },
        '10',
      ];
      const converted = convertConditionArguments('GTE', args);

      expect(converted).toEqual([
        {
          op: 'LENGTH',
          arguments: [{ op: 'CONCAT', arguments: ['Hello', 'World'] }],
        },
        10, // Should be converted to number
      ]);
    });
  });
});

describe('convertConditionArguments fidelity (finding 16)', () => {
  it('preserves type hint and default on reference arguments', () => {
    const args = [
      {
        valueType: 'reference',
        value: 'steps.fetch.outputs.count',
        type: 'number',
        default: 0,
      },
      { valueType: 'immediate', value: '5', immediateType: 'number' },
    ];
    const result = convertConditionArguments('GT', args);
    expect(result[0]).toEqual({
      valueType: 'reference',
      value: 'steps.fetch.outputs.count',
      type: 'number',
      default: 0,
    });
  });

  it('passes template arguments through untouched', () => {
    const template = {
      valueType: 'template',
      value: '{{ data.user.tier }}',
    };
    const result = convertConditionArguments('EQ', [
      template,
      { valueType: 'immediate', value: 'gold' },
    ]);
    expect(result[0]).toEqual(template);
    expect(result[0].valueType).toBe('template');
  });

  it('passes composite arguments through untouched', () => {
    const composite = {
      valueType: 'composite',
      value: { a: { valueType: 'reference', value: 'data.a' } },
    };
    const result = convertConditionArguments('EQ', [
      composite,
      { valueType: 'immediate', value: 'x' },
    ]);
    expect(result[0]).toEqual(composite);
  });

  it('honors the explicit boolean immediate type selector', () => {
    const result = convertConditionArguments('EQ', [
      { valueType: 'reference', value: 'data.isActive' },
      { valueType: 'immediate', value: 'true', immediateType: 'boolean' },
    ]);
    expect(result[1]).toEqual({ valueType: 'immediate', value: true });
  });

  it('does not stringify already-typed immediates from stored definitions', () => {
    const args = [
      { valueType: 'reference', value: 'data.flag' },
      { valueType: 'immediate', value: true },
    ];
    const result = convertConditionArguments('EQ', args);
    expect(result[1]).toEqual({ valueType: 'immediate', value: true });

    const numeric = convertConditionArguments('EQ', [
      { valueType: 'reference', value: 'data.count' },
      { valueType: 'immediate', value: 42 },
    ]);
    expect(numeric[1]).toEqual({ valueType: 'immediate', value: 42 });
  });

  it('is idempotent for stored IN arrays', () => {
    const args = [
      { valueType: 'reference', value: 'data.country' },
      { valueType: 'immediate', value: ['US', 'CA'] },
    ];
    const once = convertConditionArguments('IN', args, undefined, {
      parseInLists: true,
    });
    const twice = convertConditionArguments('IN', once, undefined, {
      parseInLists: true,
    });
    expect(once[1]).toEqual({ valueType: 'immediate', value: ['US', 'CA'] });
    expect(twice).toEqual(once);
  });

  it('parses comma-separated IN lists into arrays on the save path only', () => {
    const args = [
      { valueType: 'reference', value: 'data.country' },
      { valueType: 'immediate', value: 'US, CA, MX', immediateType: 'string' },
    ];
    const editorPath = convertConditionArguments('IN', args);
    expect(editorPath[1]).toEqual({
      valueType: 'immediate',
      value: 'US, CA, MX',
    });

    const savePath = convertConditionArguments('IN', args, undefined, {
      parseInLists: true,
    });
    expect(savePath[1]).toEqual({
      valueType: 'immediate',
      value: ['US', 'CA', 'MX'],
    });
  });

  it('parses JSON-array IN lists on the save path', () => {
    const savePath = convertConditionArguments(
      'NOT_IN',
      [
        { valueType: 'reference', value: 'data.code' },
        { valueType: 'immediate', value: '[1, 2, 3]' },
      ],
      undefined,
      { parseInLists: true }
    );
    expect(savePath[1]).toEqual({ valueType: 'immediate', value: [1, 2, 3] });
  });

  it('applies fidelity rules inside nested conditions with options', () => {
    const nested = {
      type: 'operation',
      op: 'IN',
      arguments: [
        { valueType: 'reference', value: 'data.country' },
        { valueType: 'immediate', value: 'US, CA' },
      ],
    };
    const result = convertConditionArguments('AND', [nested], undefined, {
      parseInLists: true,
    });
    expect(result[0].arguments[1]).toEqual({
      valueType: 'immediate',
      value: ['US', 'CA'],
    });
  });
});
