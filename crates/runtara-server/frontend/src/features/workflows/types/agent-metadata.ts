/**
 * Enhanced types and utilities for agent capability metadata-driven UI rendering
 */

import type { CapabilityField } from '@/generated/RuntaraRuntimeApi';

/**
 * Field value priority for determining what to display/use
 */
export const getFieldInitialValue = (
  field: CapabilityField,
  userValue?: any
): any => {
  // Priority: user input > default > example > empty
  if (userValue !== undefined && userValue !== null && userValue !== '') {
    return userValue;
  }
  if (field.default !== undefined && field.default !== null) {
    return field.default;
  }
  if (field.example !== undefined && field.example !== null) {
    return field.example;
  }
  return '';
};

/**
 * Get help text for a field, including default value indicator
 */
export const getFieldHelpText = (field: CapabilityField): string => {
  let helpText = field.description || '';

  if (field.default !== undefined && field.default !== null) {
    const defaultStr =
      typeof field.default === 'string'
        ? field.default
        : JSON.stringify(field.default);
    helpText += helpText
      ? ` (default: ${defaultStr})`
      : `Default: ${defaultStr}`;
  }

  return helpText;
};

/**
 * Get placeholder text for a field
 */
export const getFieldPlaceholder = (field: CapabilityField): string => {
  if (field.default !== undefined && field.default !== null) {
    const defaultStr =
      typeof field.default === 'string'
        ? field.default
        : JSON.stringify(field.default);
    return `Default: ${defaultStr}`;
  }
  if (field.example !== undefined && field.example !== null) {
    const exampleStr =
      typeof field.example === 'string'
        ? field.example
        : JSON.stringify(field.example);
    return exampleStr;
  }
  return '';
};

/**
 * Get display label for a field
 */
export const getFieldLabel = (field: CapabilityField): string => {
  return field.displayName || field.name;
};

/**
 * Check if a field is an enum type
 */
export const isEnumField = (field: CapabilityField): boolean => {
  return Array.isArray(field.enum) && field.enum.length > 0;
};

/**
 * Check if a field is a boolean type
 */
const isBooleanField = (field: CapabilityField): boolean => {
  return field.type === 'boolean';
};

/**
 * Check if a field is a numeric type
 */
const isNumericField = (field: CapabilityField): boolean => {
  return field.type === 'number' || field.type === 'integer';
};

/**
 * Check if a field is an object/JSON type
 */
const isObjectField = (field: CapabilityField): boolean => {
  return field.type === 'object' || field.type === 'array';
};

/**
 * Check if a field is a file type
 */
const isFileField = (field: CapabilityField): boolean => {
  return field.type === 'file' || field.type === 'File';
};

/**
 * Map backend field type to frontend input component type
 */
export type InputComponentType =
  | 'text'
  | 'number'
  | 'boolean'
  | 'select'
  | 'json'
  | 'textarea'
  | 'file';

export const getInputComponentType = (
  field: CapabilityField
): InputComponentType => {
  if (isFileField(field)) return 'file';
  if (isEnumField(field)) return 'select';
  if (isBooleanField(field)) return 'boolean';
  if (isNumericField(field)) return 'number';
  if (isObjectField(field)) return 'json';

  // Check if it's likely to be multi-line text
  if (
    field.name.includes('template') ||
    field.name.includes('text') ||
    field.name.includes('prompt')
  ) {
    return 'textarea';
  }

  return 'text';
};

/**
 * Parse field value from user input
 */
const parseFieldValue = (input: string, field: CapabilityField): any => {
  if (!input) return undefined;

  if (isNumericField(field)) {
    const num = Number(input);
    return isNaN(num) ? undefined : num;
  }

  if (isBooleanField(field)) {
    return input === 'true' || input === '1';
  }

  if (isObjectField(field)) {
    try {
      return JSON.parse(input);
    } catch {
      return input; // Return as string if parsing fails
    }
  }

  return input;
};

/**
 * Parse test agent inputs from form values
 * Converts JSON string values to actual arrays/objects based on field metadata
 */
export const parseTestAgentInputs = (
  formValues: Record<string, any>,
  fields: CapabilityField[]
): Record<string, any> => {
  const parsedInputs: Record<string, any> = {};

  for (const field of fields) {
    const value = formValues[field.name];

    // Skip undefined/null values for optional fields
    if (
      !field.required &&
      (value === undefined || value === null || value === '')
    ) {
      continue;
    }

    // Parse value based on field type
    parsedInputs[field.name] = parseFieldValue(String(value), field);
  }

  return parsedInputs;
};
