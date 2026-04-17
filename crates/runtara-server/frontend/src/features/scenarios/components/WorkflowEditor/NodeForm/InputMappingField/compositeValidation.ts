/**
 * Validation utilities for composite values.
 * Provides recursive validation of composite structures including
 * reference path validation within nested objects and arrays.
 */

import type {
  CompositeValue,
  CompositeObjectValue,
  CompositeArrayValue,
} from '@/features/scenarios/stores/nodeFormStore';

export interface ValidationError {
  /** Path to the invalid value (e.g., "field1.subfield" or "[0].name") */
  path: string;
  /** Error message */
  message: string;
  /** Error type */
  type: 'invalid_reference' | 'empty_reference' | 'invalid_structure';
}

export interface ValidationResult {
  isValid: boolean;
  errors: ValidationError[];
}

/**
 * Validates a composite value structure.
 *
 * @param value - The composite value to validate
 * @param availablePaths - Set of valid reference paths (optional, for reference validation)
 * @param basePath - Base path for error reporting (internal use)
 * @returns Validation result with any errors found
 */
export function validateCompositeValue(
  value: CompositeObjectValue | CompositeArrayValue,
  availablePaths?: Set<string>,
  basePath = ''
): ValidationResult {
  const errors: ValidationError[] = [];

  if (Array.isArray(value)) {
    // Array composite
    value.forEach((item, index) => {
      const itemPath = basePath ? `${basePath}[${index}]` : `[${index}]`;
      const itemErrors = validateCompositeItem(item, availablePaths, itemPath);
      errors.push(...itemErrors);
    });
  } else {
    // Object composite
    Object.entries(value).forEach(([key, item]) => {
      const itemPath = basePath ? `${basePath}.${key}` : key;
      const itemErrors = validateCompositeItem(item, availablePaths, itemPath);
      errors.push(...itemErrors);
    });
  }

  return {
    isValid: errors.length === 0,
    errors,
  };
}

/**
 * Validates a single composite item (which can be immediate, reference, or nested composite).
 */
function validateCompositeItem(
  item: CompositeValue,
  availablePaths?: Set<string>,
  path: string = ''
): ValidationError[] {
  const errors: ValidationError[] = [];

  if (!item || typeof item !== 'object' || !('valueType' in item)) {
    errors.push({
      path,
      message: 'Invalid structure: missing valueType',
      type: 'invalid_structure',
    });
    return errors;
  }

  switch (item.valueType) {
    case 'immediate':
      // Immediate values are always valid (type checking is done at runtime)
      break;

    case 'reference':
      if (!item.value || typeof item.value !== 'string') {
        errors.push({
          path,
          message: 'Reference path is empty',
          type: 'empty_reference',
        });
      } else if (
        availablePaths &&
        !isValidReferencePath(item.value, availablePaths)
      ) {
        errors.push({
          path,
          message: `Invalid reference: "${item.value}"`,
          type: 'invalid_reference',
        });
      }
      break;

    case 'composite':
      if (!item.value || typeof item.value !== 'object') {
        errors.push({
          path,
          message: 'Invalid composite: value must be an object or array',
          type: 'invalid_structure',
        });
      } else {
        const nestedResult = validateCompositeValue(
          item.value as CompositeObjectValue | CompositeArrayValue,
          availablePaths,
          path
        );
        errors.push(...nestedResult.errors);
      }
      break;

    default:
      errors.push({
        path,
        message: `Unknown valueType: ${(item as any).valueType}`,
        type: 'invalid_structure',
      });
  }

  return errors;
}

/**
 * Checks if a reference path is valid.
 * A path is valid if it matches one of the available paths or is a prefix of one.
 */
function isValidReferencePath(
  path: string,
  availablePaths: Set<string>
): boolean {
  // Direct match
  if (availablePaths.has(path)) {
    return true;
  }

  // Check if path is a valid prefix (accessing a nested property)
  for (const availablePath of availablePaths) {
    if (
      availablePath.startsWith(path + '.') ||
      availablePath.startsWith(path + '[')
    ) {
      return true;
    }
    // Check if input path is a deeper access into an available path
    if (
      path.startsWith(availablePath + '.') ||
      path.startsWith(availablePath + '[')
    ) {
      return true;
    }
  }

  // Special cases: data.*, steps['*'], variables.* are generally valid patterns
  if (
    path.startsWith('data.') ||
    path.startsWith("steps['") ||
    path.startsWith('variables.')
  ) {
    return true;
  }

  return false;
}
