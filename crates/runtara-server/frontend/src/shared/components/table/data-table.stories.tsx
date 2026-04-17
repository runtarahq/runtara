import type { Meta, StoryObj } from '@storybook/react';
import { useState } from 'react';
import { ColumnDef, SortingState } from '@tanstack/react-table';
import { DataTable } from './index';
import { Badge } from '../ui/badge';
import { Button } from '../ui/button';
import { Checkbox } from '../ui/checkbox';
import { ChevronDown, ChevronRight } from 'lucide-react';

// Sample data types
interface User {
  id: string;
  name: string;
  email: string;
  status: 'active' | 'inactive' | 'pending';
  role: string;
  createdAt: string;
}

interface Order {
  id: string;
  customer: string;
  total: number;
  status: 'pending' | 'processing' | 'shipped' | 'delivered';
  items: { name: string; quantity: number; price: number }[];
}

// Sample data
const sampleUsers: User[] = [
  {
    id: '1',
    name: 'John Doe',
    email: 'john@example.com',
    status: 'active',
    role: 'Admin',
    createdAt: '2024-01-15',
  },
  {
    id: '2',
    name: 'Jane Smith',
    email: 'jane@example.com',
    status: 'active',
    role: 'User',
    createdAt: '2024-01-20',
  },
  {
    id: '3',
    name: 'Bob Wilson',
    email: 'bob@example.com',
    status: 'inactive',
    role: 'User',
    createdAt: '2024-02-01',
  },
  {
    id: '4',
    name: 'Alice Brown',
    email: 'alice@example.com',
    status: 'pending',
    role: 'Editor',
    createdAt: '2024-02-10',
  },
  {
    id: '5',
    name: 'Charlie Davis',
    email: 'charlie@example.com',
    status: 'active',
    role: 'User',
    createdAt: '2024-02-15',
  },
];

const manyUsers: User[] = Array.from({ length: 50 }, (_, i) => ({
  id: String(i + 1),
  name: `User ${i + 1}`,
  email: `user${i + 1}@example.com`,
  status: ['active', 'inactive', 'pending'][i % 3] as User['status'],
  role: ['Admin', 'User', 'Editor'][i % 3],
  createdAt: `2024-0${(i % 12) + 1}-${String((i % 28) + 1).padStart(2, '0')}`,
}));

const sampleOrders: Order[] = [
  {
    id: 'ORD-001',
    customer: 'John Doe',
    total: 299.99,
    status: 'shipped',
    items: [
      { name: 'Widget A', quantity: 2, price: 49.99 },
      { name: 'Widget B', quantity: 1, price: 200.01 },
    ],
  },
  {
    id: 'ORD-002',
    customer: 'Jane Smith',
    total: 149.5,
    status: 'processing',
    items: [{ name: 'Gadget X', quantity: 3, price: 49.83 }],
  },
  {
    id: 'ORD-003',
    customer: 'Bob Wilson',
    total: 599.0,
    status: 'delivered',
    items: [
      { name: 'Device Pro', quantity: 1, price: 499.0 },
      { name: 'Accessory', quantity: 2, price: 50.0 },
    ],
  },
];

// Column definitions
const userColumns: ColumnDef<User>[] = [
  {
    accessorKey: 'name',
    header: 'Name',
    enableSorting: true,
  },
  {
    accessorKey: 'email',
    header: 'Email',
    enableSorting: true,
  },
  {
    accessorKey: 'status',
    header: 'Status',
    cell: ({ row }) => {
      const status = row.getValue('status') as string;
      const variant =
        status === 'active'
          ? 'success'
          : status === 'pending'
            ? 'warning'
            : 'secondary';
      return <Badge variant={variant}>{status}</Badge>;
    },
  },
  {
    accessorKey: 'role',
    header: 'Role',
  },
  {
    accessorKey: 'createdAt',
    header: 'Created',
    enableSorting: true,
  },
];

const selectableUserColumns: ColumnDef<User>[] = [
  {
    id: 'select',
    header: ({ table }) => (
      <Checkbox
        checked={table.getIsAllPageRowsSelected()}
        onCheckedChange={(value) => table.toggleAllPageRowsSelected(!!value)}
        aria-label="Select all"
      />
    ),
    cell: ({ row }) => (
      <Checkbox
        checked={row.getIsSelected()}
        onCheckedChange={(value) => row.toggleSelected(!!value)}
        aria-label="Select row"
      />
    ),
    size: 40,
  },
  ...userColumns,
];

const orderColumns: ColumnDef<Order>[] = [
  {
    id: 'expander',
    header: () => null,
    cell: ({ row }) => (
      <Button
        variant="ghost"
        size="sm"
        onClick={() => row.toggleExpanded()}
        className="p-0 h-6 w-6"
      >
        {row.getIsExpanded() ? (
          <ChevronDown className="h-4 w-4" />
        ) : (
          <ChevronRight className="h-4 w-4" />
        )}
      </Button>
    ),
    size: 40,
  },
  {
    accessorKey: 'id',
    header: 'Order ID',
  },
  {
    accessorKey: 'customer',
    header: 'Customer',
  },
  {
    accessorKey: 'total',
    header: 'Total',
    cell: ({ row }) => `$${row.getValue<number>('total').toFixed(2)}`,
  },
  {
    accessorKey: 'status',
    header: 'Status',
    cell: ({ row }) => {
      const status = row.getValue('status') as string;
      const variant =
        status === 'delivered'
          ? 'success'
          : status === 'shipped'
            ? 'default'
            : status === 'processing'
              ? 'warning'
              : 'secondary';
      return <Badge variant={variant}>{status}</Badge>;
    },
  },
];

const meta: Meta<typeof DataTable> = {
  title: 'Tables/DataTable',
  component: DataTable,
  parameters: {
    layout: 'padded',
    docs: {
      description: {
        component:
          'A full-featured data table built on TanStack Table. Supports sorting, pagination, row selection, expandable rows, and custom cell rendering.',
      },
    },
  },
  tags: ['autodocs'],
};

export default meta;
type Story = StoryObj<typeof DataTable>;

export const Default: Story = {
  args: {
    columns: userColumns as ColumnDef<unknown, unknown>[],
    data: sampleUsers,
    shouldRenderPagination: false,
  },
};

const WithPaginationRender = () => {
  const [pagination, setPagination] = useState({
    pageIndex: 0,
    pageSize: 10,
  });

  const pageCount = Math.ceil(manyUsers.length / pagination.pageSize);
  const paginatedData = manyUsers.slice(
    pagination.pageIndex * pagination.pageSize,
    (pagination.pageIndex + 1) * pagination.pageSize
  );

  return (
    <DataTable
      columns={userColumns}
      data={paginatedData}
      pagination={{
        pageIndex: pagination.pageIndex,
        pageSize: pagination.pageSize,
        pageCount,
        totalCount: manyUsers.length,
        onPageChange: (page) =>
          setPagination((p) => ({ ...p, pageIndex: page })),
        onPageSizeChange: (size) =>
          setPagination({ pageIndex: 0, pageSize: size }),
      }}
    />
  );
};

export const WithPagination: Story = {
  name: 'With Pagination',
  render: () => <WithPaginationRender />,
};

const WithSortingRender = () => {
  const [sorting, setSorting] = useState<SortingState>([]);
  const [data, setData] = useState(sampleUsers);

  const handleSortingChange = (
    updaterOrValue: SortingState | ((old: SortingState) => SortingState)
  ) => {
    const newSorting =
      typeof updaterOrValue === 'function'
        ? updaterOrValue(sorting)
        : updaterOrValue;
    setSorting(newSorting);
    if (newSorting.length > 0) {
      const { id, desc } = newSorting[0];
      const sorted = [...sampleUsers].sort((a, b) => {
        const aVal = a[id as keyof User];
        const bVal = b[id as keyof User];
        if (aVal < bVal) return desc ? 1 : -1;
        if (aVal > bVal) return desc ? -1 : 1;
        return 0;
      });
      setData(sorted);
    } else {
      setData(sampleUsers);
    }
  };

  return (
    <DataTable
      columns={userColumns}
      data={data}
      sorting={sorting}
      onSortingChange={
        handleSortingChange as (
          sorting: SortingState | ((old: SortingState) => SortingState)
        ) => void
      }
      manualSorting
      shouldRenderPagination={false}
    />
  );
};

export const WithSorting: Story = {
  name: 'With Sorting',
  render: () => <WithSortingRender />,
};

const WithRowSelectionRender = () => {
  const [rowSelection, setRowSelection] = useState({});

  return (
    <div className="space-y-4">
      <DataTable
        columns={selectableUserColumns}
        data={sampleUsers}
        enableRowSelection
        rowSelection={rowSelection}
        onRowSelectionChange={setRowSelection}
        getRowId={(row) => row.id}
        shouldRenderPagination={false}
      />
      <div className="p-3 bg-slate-100 dark:bg-slate-800 rounded text-sm">
        <strong>Selected:</strong> {Object.keys(rowSelection).length} rows
        {Object.keys(rowSelection).length > 0 && (
          <span className="ml-2 text-muted-foreground">
            (IDs: {Object.keys(rowSelection).join(', ')})
          </span>
        )}
      </div>
    </div>
  );
};

export const WithRowSelection: Story = {
  name: 'With Row Selection',
  render: () => <WithRowSelectionRender />,
};

export const ExpandableRows: Story = {
  name: 'Expandable Rows',
  render: () => (
    <DataTable
      columns={orderColumns}
      data={sampleOrders}
      getRowCanExpand={() => true}
      SubComponent={({ row }) => (
        <div className="p-4 bg-muted/30 rounded">
          <h4 className="font-medium mb-2">Order Items</h4>
          <table className="w-full text-sm">
            <thead>
              <tr className="text-muted-foreground">
                <th className="text-left py-1">Item</th>
                <th className="text-right py-1">Qty</th>
                <th className="text-right py-1">Price</th>
              </tr>
            </thead>
            <tbody>
              {row.original.items.map((item, i) => (
                <tr key={i}>
                  <td className="py-1">{item.name}</td>
                  <td className="text-right py-1">{item.quantity}</td>
                  <td className="text-right py-1">${item.price.toFixed(2)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      shouldRenderPagination={false}
    />
  ),
};

export const Loading: Story = {
  args: {
    columns: userColumns as ColumnDef<unknown, unknown>[],
    data: [],
    isFetching: true,
  },
};

export const Empty: Story = {
  args: {
    columns: userColumns as ColumnDef<unknown, unknown>[],
    data: [],
    shouldRenderPagination: false,
  },
};

export const NestedTable: Story = {
  name: 'Nested Table Style',
  args: {
    columns: userColumns as ColumnDef<unknown, unknown>[],
    data: sampleUsers.slice(0, 3),
    isNested: true,
    shouldRenderPagination: false,
  },
};

export const CustomRowClassName: Story = {
  name: 'Custom Row Styling',
  render: () => (
    <DataTable
      columns={userColumns}
      data={sampleUsers}
      getRowClassName={(row) =>
        row.original.status === 'inactive' ? 'opacity-50' : ''
      }
      shouldRenderPagination={false}
    />
  ),
};
