/**
 * Conversion of step input mappings between the DSL shape
 * (`{valueType, value, type?, default?}`) and the editor's UI shape
 * (`typeHint`, `defaultValue`), shared by the workflow canvas and the report
 * layout editor.
 */
import type { ValueType } from '@/generated/RuntaraRuntimeApi.ts';

// Valid ValueType values from the spec
const VALID_VALUE_TYPES: ReadonlySet<ValueType> = new Set<ValueType>([
  'string',
  'integer',
  'number',
  'boolean',
  'json',
  'file',
]);

/**
 * Coerces a value to match the given type hint (using API ValueType convention).
 * e.g., "150" with type "integer" becomes 150
 */
export function coerceValueToType(value: any, typeHint?: string): any {
  if (typeHint === 'integer' || typeHint === 'number') {
    const numValue = Number(value);
    if (!isNaN(numValue)) {
      return typeHint === 'integer' ? Math.trunc(numValue) : numValue;
    }
  }
  if (typeHint === 'boolean' && typeof value === 'string') {
    const lower = value.toLowerCase();
    if (lower === 'true' || lower === '1') return true;
    if (lower === 'false' || lower === '0') return false;
  }
  return value;
}

// Check if a typeHint is a valid ValueType
export function isValidValueType(typeHint?: string): typeHint is ValueType {
  return typeHint !== undefined && VALID_VALUE_TYPES.has(typeHint as ValueType);
}

// Helper function to recursively process composite values.
// Mirror of convertCompositeToUIFormat below — preserves typeHint/defaultValue for
// every non-composite valueType (not only `immediate`) so the UI→backend round-trip is lossless.
export const processCompositeValue = (
  compositeVal: any
): {
  valueType: 'reference' | 'immediate' | 'composite';
  value: any;
} => {
  const processEntry = (val: any) => {
    if (
      typeof val !== 'object' ||
      val === null ||
      !('valueType' in (val as Record<string, unknown>))
    ) {
      return {
        valueType: 'immediate',
        value: val,
      };
    }

    const typedVal = val as {
      valueType: 'reference' | 'immediate' | 'composite' | 'template';
      value: any;
      type?: string;
      typeHint?: string;
      default?: any;
      defaultValue?: any;
    };

    if (typedVal.valueType === 'composite') {
      const nestedValue =
        typedVal.value && typeof typedVal.value === 'object'
          ? typedVal.value
          : {};
      return {
        valueType: 'composite',
        value: processCompositeValue(nestedValue).value,
      };
    }

    const coercedValue =
      typedVal.valueType === 'immediate' &&
      typedVal.typeHint &&
      typedVal.value !== null
        ? coerceValueToType(typedVal.value, typedVal.typeHint)
        : typedVal.value === undefined
          ? ''
          : typedVal.value;

    const out: {
      valueType: string;
      value: any;
      type?: string;
      default?: any;
    } = {
      valueType: typedVal.valueType || 'immediate',
      value: coercedValue,
    };
    const typeHint = typedVal.typeHint ?? typedVal.type;
    if (typedVal.valueType === 'reference' && isValidValueType(typeHint)) {
      out.type = typeHint;
    }
    const defaultValue = typedVal.defaultValue ?? typedVal.default;
    if (typedVal.valueType === 'reference' && defaultValue !== undefined) {
      out.default = defaultValue;
    }
    return out;
  };

  // Handle composite object
  if (
    compositeVal &&
    typeof compositeVal === 'object' &&
    !Array.isArray(compositeVal)
  ) {
    const processedObject: Record<string, any> = {};
    for (const [key, val] of Object.entries(compositeVal)) {
      processedObject[key] = processEntry(val);
    }
    return { valueType: 'composite', value: processedObject };
  }

  // Handle composite array
  if (Array.isArray(compositeVal)) {
    return {
      valueType: 'composite',
      value: compositeVal.map(processEntry),
    };
  }

  // Fallback - shouldn't happen for properly structured data
  return { valueType: 'immediate', value: compositeVal };
};

// Helper function to process a single mapping entry
export const processMappingEntry = ({
  type,
  value,
  typeHint,
  valueType,
  defaultValue,
}: {
  type: string;
  value: any;
  typeHint?: string;
  valueType?: 'reference' | 'immediate' | 'composite' | 'template';
  defaultValue?: any;
}) => {
  // Handle template values - always a string, no type coercion
  if (valueType === 'template') {
    return [type, { valueType: 'template', value: String(value) }];
  }

  // Handle composite values - process recursively
  if (valueType === 'composite') {
    const processed = processCompositeValue(value);
    const mappingValue: {
      valueType: 'composite';
      value: any;
    } = {
      valueType: 'composite',
      value: processed.value,
    };
    return [type, mappingValue];
  }

  // Parse JSON strings into actual arrays/objects before sending to backend
  let finalValue = value;

  if (typeof value === 'string' && value) {
    // Skip parsing for template variables (they're resolved at runtime)
    const isTemplate = value.includes('{{');

    if (!isTemplate) {
      // For non-template strings, only parse as JSON if the typeHint is
      // explicitly JSON-shaped. 'object'/'array' are form-level hints
      // (e.g. Finish output types) that keep the editors' object-vs-array
      // distinction; they carry the same parse semantics as 'json' and
      // are never emitted as backend type hints (isValidValueType).
      // No auto-detection - explicit typeHint required.
      if (
        typeHint === 'json' ||
        typeHint === 'object' ||
        typeHint === 'array'
      ) {
        try {
          finalValue = JSON.parse(value);
        } catch {
          // If parsing fails, keep as string
          finalValue = value;
        }
      }

      // Convert numeric strings to actual numbers for integer/number type hints
      if (typeHint === 'integer' || typeHint === 'number') {
        const numValue = Number(value);
        if (!isNaN(numValue)) {
          // For integers, ensure we get a whole number
          finalValue = typeHint === 'integer' ? Math.trunc(numValue) : numValue;
        }
      }

      // Convert boolean strings to actual booleans for boolean type hint
      if (typeHint === 'boolean') {
        const lowerValue = value.toLowerCase();
        if (lowerValue === 'true' || lowerValue === '1') {
          finalValue = true;
        } else if (lowerValue === 'false' || lowerValue === '0') {
          finalValue = false;
        }
      }
    }
  }

  // Use explicit valueType from UI, fallback to auto-detection for backward compatibility
  const resolvedValueType: 'reference' | 'immediate' | 'template' =
    valueType ||
    (typeof finalValue === 'string' && finalValue.includes('{{')
      ? 'reference'
      : 'immediate');

  // Create the new format per DSL v2.0.0 spec: { valueType, value, type?, default? }
  const mappingValue: {
    valueType: 'reference' | 'immediate' | 'template';
    value: any;
    type?: string;
    default?: any;
  } = {
    valueType: resolvedValueType,
    value: finalValue,
  };

  // Only reference values carry backend type hints. Immediate, composite,
  // and template values reject unknown `type` fields.
  if (resolvedValueType === 'reference' && isValidValueType(typeHint)) {
    mappingValue.type = typeHint;
  }

  // Preserve ReferenceValue.default — only references carry this field on the backend.
  if (resolvedValueType === 'reference' && defaultValue !== undefined) {
    mappingValue.default = defaultValue;
  }

  return [type, mappingValue];
};

// Helper function to convert composite values from API format (type) to UI format (typeHint)
export const convertCompositeToUIFormat = (compositeVal: any): any => {
  const convertEntry = (val: any) => {
    const typedVal = val as {
      valueType: 'reference' | 'immediate' | 'composite' | 'template';
      value: any;
      type?: string;
      default?: any;
    };
    if (typedVal.valueType === 'composite') {
      return {
        valueType: 'composite',
        value: convertCompositeToUIFormat(typedVal.value),
        ...(typedVal.type ? { typeHint: typedVal.type } : {}),
      };
    }
    const out: Record<string, any> = {
      valueType: typedVal.valueType,
      value: typedVal.value,
    };
    // Convert backend `type` → UI `typeHint` for every non-composite variant,
    // not only `immediate` — references/templates can carry type hints too.
    if (typedVal.type !== undefined) {
      out.typeHint = typedVal.type;
    }
    // Preserve ReferenceValue.default so it survives the UI round-trip.
    if (typedVal.valueType === 'reference' && typedVal.default !== undefined) {
      out.defaultValue = typedVal.default;
    }
    return out;
  };

  // Handle composite object
  if (
    compositeVal &&
    typeof compositeVal === 'object' &&
    !Array.isArray(compositeVal)
  ) {
    const convertedObject: Record<string, any> = {};
    for (const [key, val] of Object.entries(compositeVal)) {
      convertedObject[key] = convertEntry(val);
    }
    return convertedObject;
  }

  // Handle composite array
  if (Array.isArray(compositeVal)) {
    return compositeVal.map(convertEntry);
  }

  // Return as-is if not a composite structure
  return compositeVal;
};
