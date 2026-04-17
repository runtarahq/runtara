import { Card } from '@/shared/components/ui/card';

interface SQLPreviewProps {
  schemaName: string;
  tableName: string;
  columns: Array<{
    name: string;
    type: string;
    nullable?: boolean;
    unique?: boolean;
    default?: string;
  }>;
  indexes?: Array<{
    name: string;
    columns: string[];
    unique?: boolean;
  }>;
}

export function SQLPreview({
  schemaName,
  tableName,
  columns,
  indexes = [],
}: SQLPreviewProps) {
  const generateSQL = () => {
    if (!tableName || columns.length === 0) {
      return '-- Configure schema name, table name, and at least one column to see preview';
    }

    const lines: string[] = [];
    lines.push(`-- Schema: ${schemaName || 'Unnamed'}`);
    lines.push(`CREATE TABLE ${tableName} (`);

    // Add columns
    const columnDefs = columns
      .filter((col) => col.name)
      .map((col, idx, arr) => {
        const parts = [`  ${col.name} ${col.type}`];

        if (!col.nullable) {
          parts.push('NOT NULL');
        }

        if (col.unique) {
          parts.push('UNIQUE');
        }

        if (col.default) {
          parts.push(`DEFAULT ${col.default}`);
        }

        const isLast = idx === arr.length - 1 && indexes.length === 0;
        return parts.join(' ') + (isLast ? '' : ',');
      });

    lines.push(...columnDefs);

    // Add indexes as table constraints
    if (indexes.length > 0) {
      indexes.forEach((index, idx) => {
        if (index.name && index.columns.length > 0) {
          const uniqueKeyword = index.unique ? 'UNIQUE ' : '';
          const columnList = index.columns.join(', ');
          const isLast = idx === indexes.length - 1;
          lines.push(
            `  ${uniqueKeyword}INDEX ${index.name} (${columnList})${isLast ? '' : ','}`
          );
        }
      });
    }

    lines.push(');');

    return lines.join('\n');
  };

  return (
    <div className="space-y-3">
      <div>
        <h3 className="text-sm font-semibold text-foreground">SQL Preview</h3>
        <p className="mt-1 text-xs text-muted-foreground">
          Generated SQL for your schema (read-only)
        </p>
      </div>
      <Card className="overflow-hidden rounded-2xl border border-border/40 bg-muted/20">
        <pre className="overflow-x-auto p-4 text-xs font-mono text-foreground/90">
          <code>{generateSQL()}</code>
        </pre>
      </Card>
    </div>
  );
}
