/**
 * Utility functions for working with condition expressions.
 * Used by ConditionalNode for display and InputMappingField for editing.
 */

interface InputMappingEntry {
  type: string;
  value?: string | number | boolean | null | any[] | object;
  typeHint?: string;
  valueType?: 'reference' | 'immediate' | 'composite' | 'template';
}

// Argument with value type metadata (for reference vs immediate values)
interface ConditionArgument {
  valueType: 'immediate' | 'reference';
  value: string;
}

/**
 * Reconstructs a condition object from flattened inputMapping entries.
 *
 * The backend stores conditions in a flattened format with keys like:
 * - 'condition.expression.op' -> 'EQ'
 * - 'condition.expression.arguments[0]' -> '{{order.status}}'
 * - 'condition.expression.arguments[1]' -> 'shipped'
 *
 * This function rebuilds the nested condition object structure.
 */
export function getConditionFromInputMapping(
  inputMapping: InputMappingEntry[]
): { op: string; arguments: any[] } | null {
  if (
    !inputMapping ||
    !Array.isArray(inputMapping) ||
    inputMapping.length === 0
  ) {
    return null;
  }

  // Start with the condition structure expected by the backend
  const result: any = {};

  // Track valueType for arguments by their index path
  const argumentValueTypes: Map<
    string,
    'reference' | 'immediate' | 'composite' | 'template'
  > = new Map();

  // Process each field in the input mapping
  for (const field of inputMapping) {
    let type = field.type;
    const value = field.value;
    const valueType = field.valueType;

    // Skip empty fields
    if (!type) continue;

    // Normalize the type path by removing 'condition.expression.' prefix if present
    // Backend sends: 'condition.expression.op' -> we want to build: {op: ...}
    if (type.startsWith('condition.expression.')) {
      type = type.substring('condition.expression.'.length);
    } else if (type.startsWith('expression.')) {
      // Also handle old format without 'condition.' prefix
      type = type.substring('expression.'.length);
    } else {
      // Skip entries that don't match the condition pattern
      continue;
    }

    // Track valueType for argument entries (e.g., 'arguments[0]')
    if (valueType && type.match(/^arguments\[\d+\]$/)) {
      argumentValueTypes.set(type, valueType);
    }

    // Split the type into path segments
    const pathSegments = type.split('.');

    // Build the nested object structure
    let current = result;
    for (let i = 0; i < pathSegments.length - 1; i++) {
      const segment = pathSegments[i];

      // Check if the segment contains an array index
      const arrayMatch = segment.match(/^([^[]+)\[(\d+)\]$/);

      if (arrayMatch) {
        // This is an array segment like "arguments[0]"
        const arrayName = arrayMatch[1];
        const arrayIndex = parseInt(arrayMatch[2], 10);

        // Create the array if it doesn't exist
        if (!current[arrayName]) {
          current[arrayName] = [];
        }

        // Ensure the array is large enough
        while (current[arrayName].length <= arrayIndex) {
          current[arrayName].push({});
        }

        // Move to the array element
        current = current[arrayName][arrayIndex];
      } else {
        // Regular object property
        if (!current[segment]) {
          current[segment] = {};
        }
        current = current[segment];
      }
    }

    // Set the value at the leaf node
    const lastSegment = pathSegments[pathSegments.length - 1];

    // Check if the last segment contains an array index
    const arrayMatch = lastSegment.match(/^([^[]+)\[(\d+)\]$/);

    if (arrayMatch) {
      // This is an array segment like "arguments[0]"
      const arrayName = arrayMatch[1];
      const arrayIndex = parseInt(arrayMatch[2], 10);

      // Create the array if it doesn't exist
      if (!current[arrayName]) {
        current[arrayName] = [];
      }

      // Ensure the array is large enough
      while (current[arrayName].length <= arrayIndex) {
        current[arrayName].push(undefined);
      }

      // Check if this argument has a valueType (for reference values)
      const argKey = lastSegment; // e.g., 'arguments[0]'
      const argValueType = argumentValueTypes.get(argKey);

      // Try to parse the value as JSON if it looks like a JSON string
      let finalValue: any;
      try {
        if (
          value &&
          typeof value === 'string' &&
          (value.startsWith('{') ||
            value.startsWith('[') ||
            value === 'true' ||
            value === 'false' ||
            !isNaN(Number(value)))
        ) {
          finalValue = JSON.parse(value);
        } else {
          finalValue = value;
        }
      } catch {
        // If parsing fails, use the raw value
        finalValue = value;
      }

      // Wrap in ConditionArgument if valueType is 'reference'
      if (argValueType === 'reference') {
        current[arrayName][arrayIndex] = {
          valueType: 'reference',
          value: String(finalValue ?? ''),
        } as ConditionArgument;
      } else {
        current[arrayName][arrayIndex] = finalValue;
      }
    } else {
      // Regular object property
      // Try to parse the value as JSON if it looks like a JSON string
      try {
        if (
          value &&
          typeof value === 'string' &&
          (value.startsWith('{') ||
            value.startsWith('[') ||
            value === 'true' ||
            value === 'false' ||
            !isNaN(Number(value)))
        ) {
          current[lastSegment] = JSON.parse(value);
        } else {
          current[lastSegment] = value;
        }
      } catch {
        // If parsing fails, use the raw value
        current[lastSegment] = value;
      }
    }
  }

  // Validate the result has required properties
  if (
    result &&
    typeof result === 'object' &&
    'op' in result &&
    'arguments' in result
  ) {
    return result as { op: string; arguments: any[] };
  }

  return null;
}
