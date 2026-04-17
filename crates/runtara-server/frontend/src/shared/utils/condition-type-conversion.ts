/**
 * Utility functions for automatic type conversion of condition operands.
 *
 * This module provides type inference and conversion for condition expressions
 * to ensure operands are saved with the correct type (number, boolean, string)
 * rather than always as strings from input fields.
 */

/**
 * List of operators that expect numeric operands for comparisons
 */
const NUMERIC_COMPARISON_OPERATORS = ['GT', 'GTE', 'LT', 'LTE'];

/**
 * List of function operators that return numeric values
 */
const NUMERIC_FUNCTION_OPERATORS = [
  'LENGTH',
  'COUNT',
  'SUM',
  'MIN',
  'MAX',
  'ABS',
  'ROUND',
  'FLOOR',
  'CEIL',
];

/**
 * List of function operators that return boolean values
 */
const BOOLEAN_FUNCTION_OPERATORS = [
  'IS_EMPTY',
  'IS_NOT_EMPTY',
  'IS_DEFINED',
  'IS_NULL',
  'IS_NOT_NULL',
];

/**
 * List of logical operators that work with boolean values
 */
const LOGICAL_OPERATORS = ['AND', 'OR', 'NOT'];

/**
 * Schema definition type for field type lookups
 */
interface FieldTypeInfo {
  name?: string;
  dataType?: string;
}

/**
 * Map a schema data type to a conversion target type.
 */
function schemaTypeToTargetType(
  dataType: string
): 'number' | 'boolean' | 'string' {
  switch (dataType) {
    case 'INTEGER':
    case 'BIGINT':
    case 'SMALLINT':
    case 'DECIMAL':
    case 'NUMERIC':
      return 'number';
    case 'BOOLEAN':
      return 'boolean';
    default:
      return 'string';
  }
}

/**
 * Infer the expected data type for a condition operand based on operator context.
 *
 * @param operator - The operator being used (e.g., 'GT', 'EQ', 'LENGTH')
 * @param argIndex - The index of the argument (0-based)
 * @param argValue - The current value of the argument (may be a nested condition)
 * @param fieldDataType - Optional schema data type for the field being compared
 * @returns The inferred type: 'number', 'boolean', or 'string'
 *
 * @example
 * inferOperandType('GT', 1, '200') // returns 'number'
 * inferOperandType('EQ', 0, 'name') // returns 'string'
 * inferOperandType('EQ', 1, '12000', 'INTEGER') // returns 'number'
 * inferOperandType('GT', 0, { op: 'LENGTH', arguments: [...] }) // returns 'number'
 */
export function inferOperandType(
  operator: string,
  argIndex: number,
  argValue: any,
  fieldDataType?: string
): 'number' | 'boolean' | 'string' {
  // If the argument is a nested condition, infer based on that condition's operator
  if (
    typeof argValue === 'object' &&
    argValue !== null &&
    'op' in argValue &&
    argValue.op
  ) {
    const nestedOp = argValue.op;

    // Check if the nested operator returns a number
    if (NUMERIC_FUNCTION_OPERATORS.includes(nestedOp)) {
      return 'number';
    }

    // Check if the nested operator returns a boolean
    if (BOOLEAN_FUNCTION_OPERATORS.includes(nestedOp)) {
      return 'boolean';
    }

    // Numeric comparison operators return boolean
    if (NUMERIC_COMPARISON_OPERATORS.includes(nestedOp)) {
      return 'boolean';
    }

    // Logical operators return boolean
    if (LOGICAL_OPERATORS.includes(nestedOp)) {
      return 'boolean';
    }

    // Default for other nested conditions
    return 'string';
  }

  // For numeric comparison operators, the second argument should be a number
  // Example: field > 100, LENGTH(field) > 200
  if (NUMERIC_COMPARISON_OPERATORS.includes(operator) && argIndex === 1) {
    return 'number';
  }

  // For IN and NOT_IN operators with the second argument, could be array or string
  // We'll leave these as strings and let the backend parse them
  if ((operator === 'IN' || operator === 'NOT_IN') && argIndex === 1) {
    return 'string'; // Could be comma-separated or JSON array
  }

  // Logical operators work with boolean conditions
  if (LOGICAL_OPERATORS.includes(operator)) {
    return 'boolean';
  }

  // For binary operators (EQ, NE, etc.) with a known field type,
  // use the schema type to determine the value argument type
  if (fieldDataType && argIndex === 1) {
    return schemaTypeToTargetType(fieldDataType);
  }

  // Default to string for all other cases
  return 'string';
}

/**
 * Convert a value to the specified type with validation.
 *
 * @param value - The value to convert (typically from an input field)
 * @param targetType - The target type to convert to
 * @returns The converted value, or the original value if conversion fails
 *
 * @example
 * convertOperandValue('200', 'number') // returns 200
 * convertOperandValue('true', 'boolean') // returns true
 * convertOperandValue('hello', 'number') // returns 'hello' (invalid number, fallback to original)
 */
export function convertOperandValue(
  value: any,
  targetType: 'number' | 'boolean' | 'string'
): any {
  // Don't convert null, undefined, or objects (nested conditions)
  if (value === null || value === undefined) {
    return value;
  }

  if (typeof value === 'object') {
    return value; // Keep nested conditions as-is
  }

  // Convert to string for processing
  const strValue = String(value);

  // Handle empty strings
  if (strValue.trim() === '') {
    return strValue;
  }

  switch (targetType) {
    case 'number': {
      // Try to parse as number
      const num = Number(strValue);
      // Only convert if it's a valid number
      if (!isNaN(num) && isFinite(num)) {
        return num;
      }
      // If conversion fails, return the original value
      // This allows template strings like "{{variable}}" to pass through
      return value;
    }

    case 'boolean': {
      const lowerValue = strValue.toLowerCase().trim();
      if (lowerValue === 'true') return true;
      if (lowerValue === 'false') return false;
      // For other values, return as-is (could be template string)
      return value;
    }

    case 'string':
    default:
      return strValue;
  }
}

/**
 * Check if an argument is a ConditionArgument with valueType metadata
 */
function isConditionArgument(
  arg: any
): arg is { valueType: 'immediate' | 'reference'; value: string } {
  return (
    typeof arg === 'object' &&
    arg !== null &&
    'valueType' in arg &&
    'value' in arg &&
    !('op' in arg)
  );
}

/**
 * Process all arguments in a condition to apply type conversion.
 * This should be called before saving a condition to ensure proper types.
 *
 * @param operator - The condition operator
 * @param args - Array of arguments (may contain nested conditions, strings, or ConditionArguments)
 * @param schemaDefinition - Optional schema definition for field type lookups
 * @returns Array of arguments with proper type conversion applied
 *
 * @example
 * const args = ['product_price', '12000'];
 * const schema = { product_price: { dataType: 'INTEGER' } };
 * const converted = convertConditionArguments('EQ', args, schema);
 * // converted = ['product_price', 12000]
 */
export function convertConditionArguments(
  operator: string,
  args: any[],
  schemaDefinition?: Record<string, FieldTypeInfo>
): any[] {
  // Resolve the field data type from the first argument (field name) for binary operators
  const fieldDataType = resolveFieldDataType(args, schemaDefinition);

  return args.map((arg, index) => {
    // Handle ConditionArgument with valueType metadata
    if (isConditionArgument(arg)) {
      // For reference types, don't do type conversion - keep as ConditionArgument
      if (arg.valueType === 'reference') {
        return arg;
      }
      // For immediate types, apply type conversion to the value
      const inferredType = inferOperandType(
        operator,
        index,
        arg.value,
        fieldDataType
      );
      const convertedValue = convertOperandValue(arg.value, inferredType);
      // If the converted value is the same type as original, return ConditionArgument
      // Otherwise, return the converted primitive value
      return { ...arg, value: convertedValue };
    }

    // Recursively handle nested conditions
    if (typeof arg === 'object' && arg !== null && 'op' in arg && arg.op) {
      return {
        ...arg,
        arguments: arg.arguments
          ? convertConditionArguments(arg.op, arg.arguments, schemaDefinition)
          : [],
      };
    }

    // Infer the type for this argument
    const inferredType = inferOperandType(operator, index, arg, fieldDataType);

    // Convert the value
    return convertOperandValue(arg, inferredType);
  });
}

/**
 * Resolve the field data type from the first argument if it's a field name
 * present in the schema definition.
 */
function resolveFieldDataType(
  args: any[],
  schemaDefinition?: Record<string, FieldTypeInfo>
): string | undefined {
  if (!schemaDefinition || args.length < 2) return undefined;
  const firstArg = args[0];
  if (typeof firstArg !== 'string') return undefined;
  return schemaDefinition[firstArg]?.dataType;
}
