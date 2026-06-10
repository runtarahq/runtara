/**
 * Derives the value-conversion type hint for Finish step output rows that are
 * bound to a workflow output schema field.
 *
 * The hint must be a member of the backend `ValueType` set
 * ('string' | 'integer' | 'number' | 'boolean' | 'json' | 'file' — see
 * VALID_VALUE_TYPES in ../CustomNodes/utils.tsx). At runtime
 * `apply_type_hint` (runtara-workflow-stdlib direct_json.rs) coerces resolved
 * reference values: a hardcoded 'string' hint stringifies numbers/booleans,
 * so schema-bound rows must derive the hint from the schema's declared type.
 * Unknown schema types return undefined — omitting the hint (pass-through)
 * is always safe, writing an illegal one is not.
 */

import type { ValueType } from '@/generated/RuntaraRuntimeApi';

export function deriveTypeHintFromSchemaType(
  schemaType: string | undefined
): ValueType | undefined {
  if (!schemaType) return undefined;
  const lowerType = schemaType.toLowerCase();

  if (lowerType === 'string' || lowerType === 'text' || lowerType === 'str') {
    return 'string';
  }
  if (lowerType === 'integer' || lowerType === 'int') {
    return 'integer';
  }
  if (
    lowerType === 'number' ||
    lowerType === 'float' ||
    lowerType === 'double'
  ) {
    return 'number';
  }
  if (lowerType === 'boolean' || lowerType === 'bool') {
    return 'boolean';
  }
  // Objects and arrays pass through as JSON at runtime
  if (
    lowerType === 'object' ||
    lowerType === 'array' ||
    lowerType.startsWith('{') ||
    lowerType.startsWith('[') ||
    lowerType.startsWith('array<') ||
    lowerType.includes('[]')
  ) {
    return 'json';
  }
  if (lowerType === 'file') {
    return 'file';
  }

  // Unknown type — omit the hint rather than writing an illegal one
  return undefined;
}
