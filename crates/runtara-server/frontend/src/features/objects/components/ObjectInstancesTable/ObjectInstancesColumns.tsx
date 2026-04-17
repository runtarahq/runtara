import { ColumnDef } from '@tanstack/react-table';
import { Checkbox } from '@/shared/components/ui/checkbox';
import { Instance, Schema } from '@/generated/RuntaraRuntimeApi';
import { EditableCell } from './EditableCell';
import { formatDate } from '@/lib/utils';
import { IdColumnCell } from '@/shared/components/table/IdColumnCell';

// Helper type matching the new spec
type ColumnDataType =
  | 'string'
  | 'integer'
  | 'boolean'
  | 'decimal'
  | 'timestamp'
  | 'json'
  | 'enum';

// Helper function to map PostgreSQL types to UI data types
const mapPostgresTypeToDataType = (pgType: string): ColumnDataType => {
  const baseType = pgType.replace(/\[\]$/, ''); // Remove array notation

  if (baseType.startsWith('VARCHAR') || baseType === 'TEXT') {
    return 'string';
  } else if (
    baseType === 'INTEGER' ||
    baseType === 'BIGINT' ||
    baseType === 'SMALLINT'
  ) {
    return 'integer';
  } else if (baseType.startsWith('DECIMAL') || baseType.startsWith('NUMERIC')) {
    return 'decimal';
  } else if (baseType === 'BOOLEAN') {
    return 'boolean';
  } else if (
    baseType === 'DATE' ||
    baseType === 'TIMESTAMP' ||
    baseType === 'TIMESTAMPTZ'
  ) {
    return 'timestamp';
  } else if (baseType === 'JSONB' || baseType === 'JSON') {
    return 'json';
  }
  return 'string';
};

interface ObjectInstancesColumnsProps {
  objectSchemaDto: Schema;
  onUpdate: (instanceId: string, data: any) => void;
  enableSelection?: boolean;
  onCellFocus?: (rowId: string) => void;
  editingCellId: string | null;
  setEditingCellId: (cellId: string | null) => void;
}

export const objectInstancesColumns = ({
  objectSchemaDto,
  onUpdate,
  enableSelection = false,
  onCellFocus,
  editingCellId,
  setEditingCellId,
}: ObjectInstancesColumnsProps): ColumnDef<Instance>[] => {
  const columns: ColumnDef<Instance>[] = [];

  // Add selection column if enabled
  if (enableSelection) {
    columns.push({
      id: 'select',
      size: 48,
      header: ({ table }) => (
        <Checkbox
          checked={
            table.getIsAllPageRowsSelected() ||
            (table.getIsSomePageRowsSelected() && 'indeterminate')
          }
          onCheckedChange={(value) => table.toggleAllPageRowsSelected(!!value)}
          aria-label="Select all"
        />
      ),
      cell: ({ row }) => {
        if (row.original.id?.startsWith('PENDING_')) return null;
        return (
          <Checkbox
            checked={row.getIsSelected()}
            onCheckedChange={(value) => row.toggleSelected(!!value)}
            aria-label="Select row"
          />
        );
      },
      enableSorting: false,
      enableHiding: false,
      meta: {
        cellClassName: 'border-r border-slate-100 dark:border-slate-800',
        headerClassName: 'border-r border-slate-100 dark:border-slate-800',
      },
    });
  }

  // Add ID column
  columns.push({
    id: '_id',
    header: 'ID',
    size: 80,
    enableSorting: false,
    enableHiding: false,
    cell: ({ row }) => <IdColumnCell id={row.original.id!} />,
    meta: {
      cellClassName: 'border-r border-slate-100 !px-3 dark:border-slate-800',
      headerClassName: 'border-r border-slate-100 !px-3 dark:border-slate-800',
    },
  });

  // Add system timestamp columns (right after ID)
  columns.push({
    id: '_createdAt',
    header: 'Created At',
    size: 180,
    enableSorting: true,
    accessorFn: (row) => row.createdAt,
    cell: ({ row }) => (
      <div className="px-3 text-muted-foreground">
        {row.original.createdAt ? formatDate(row.original.createdAt) : '—'}
      </div>
    ),
  });

  columns.push({
    id: '_updatedAt',
    header: 'Updated At',
    size: 180,
    enableSorting: true,
    accessorFn: (row) => row.updatedAt,
    cell: ({ row }) => (
      <div className="px-3 text-muted-foreground">
        {row.original.updatedAt ? formatDate(row.original.updatedAt) : '—'}
      </div>
    ),
  });

  // Add columns for each property in the schema
  if (objectSchemaDto.columns) {
    objectSchemaDto.columns.forEach((column) => {
      const dataType = mapPostgresTypeToDataType(column.type);
      // Extract enum values if applicable
      // The API definition says: values: string[] for enum type
      const enumValues = (column as any).values as string[] | undefined;

      columns.push({
        accessorFn: (row) => {
          return row.properties?.[column.name];
        },
        id: column.name,
        header: column.name,
        size: 200,
        minSize: 150,
        meta: {
          cellClassName: 'p-0',
        },
        cell: (props) => {
          const cellId = `${props.row.original.id}-${column.name}`;
          return (
            <EditableCell
              {...props}
              onUpdate={onUpdate}
              dataType={dataType}
              enumValues={enumValues}
              onFocus={() => onCellFocus?.(props.row.original.id!)}
              isEditing={editingCellId === cellId}
              setIsEditing={(editing) =>
                setEditingCellId(editing ? cellId : null)
              }
            />
          );
        },
      });
    });
  }

  return columns;
};
