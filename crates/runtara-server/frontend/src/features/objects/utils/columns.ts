import type { ColumnDefinition } from '@/generated/RuntaraRuntimeApi';

export type ColumnDataType =
  | 'string'
  | 'integer'
  | 'boolean'
  | 'decimal'
  | 'timestamp'
  | 'json'
  | 'enum';

export function isGeneratedObjectColumn(column: ColumnDefinition): boolean {
  return column.type === 'tsvector';
}

export function getWritableObjectColumns(
  columns: ColumnDefinition[] | undefined
): ColumnDefinition[] {
  return (columns ?? []).filter((column) => !isGeneratedObjectColumn(column));
}

export function mapColumnTypeToDataType(columnType: string): ColumnDataType {
  const baseType = columnType.replace(/\[\]$/, '').toLowerCase();

  if (
    baseType === 'string' ||
    baseType === 'text' ||
    baseType.startsWith('varchar')
  ) {
    return 'string';
  }
  if (
    baseType === 'integer' ||
    baseType === 'bigint' ||
    baseType === 'smallint'
  ) {
    return 'integer';
  }
  if (
    baseType === 'decimal' ||
    baseType === 'numeric' ||
    baseType.startsWith('decimal') ||
    baseType.startsWith('numeric')
  ) {
    return 'decimal';
  }
  if (baseType === 'boolean') {
    return 'boolean';
  }
  if (
    baseType === 'timestamp' ||
    baseType === 'timestamptz' ||
    baseType === 'date'
  ) {
    return 'timestamp';
  }
  if (baseType === 'json' || baseType === 'jsonb') {
    return 'json';
  }
  if (baseType === 'enum') {
    return 'enum';
  }
  return 'string';
}
