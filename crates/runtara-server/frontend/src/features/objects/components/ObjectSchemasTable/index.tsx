import { useCallback, useState, type ReactNode } from 'react';
import { useNavigate } from 'react-router';
import { toast } from 'sonner';
import { Edit2, Trash2, Database, Plus } from 'lucide-react';
import { Schema } from '@/generated/RuntaraRuntimeApi';
import { Button } from '@/shared/components/ui/button';
import { Can } from '@/shared/components/Can';
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/shared/components/ui/table';
import {
  Breadcrumb,
  ConsoleEmptyState,
  ConsoleErrorState,
  ConsoleTableShell,
  ConsoleToolbar,
  StatusPill,
  TableSkeletonRows,
  TableStatusFooter,
} from '@/shared/components/console';
import { ModalDialog } from '@/shared/components/next-dialog';
import {
  DialogClose,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/shared/components/ui/dialog';
import { Icons } from '@/shared/components/icons';
import { formatDate } from '@/lib/utils';
import { ObjectModelConnectionSelector } from '../ObjectModelConnectionSelector';
import { Spinner } from '@/shared/components/ui/spinner';
import {
  useObjectSchemaDtos,
  useDeleteObjectSchema,
} from '@/features/objects/hooks/useObjectSchemas.ts';

interface ObjectSchemaDtosTableProps {
  connectionId?: string | null;
  /** True while the connection selection is still resolving — shows the
   *  loading skeleton instead of flashing "no connection selected" before
   *  the default connection is picked. */
  connectionsLoading?: boolean;
}

export function ObjectSchemaDtosTable({
  connectionId,
  connectionsLoading = false,
}: ObjectSchemaDtosTableProps) {
  const navigate = useNavigate();
  const [deleteTarget, setDeleteTarget] = useState<Schema | null>(null);

  const {
    data: objectSchemaDtos = [],
    isLoading,
    isError,
    error,
  } = useObjectSchemaDtos(connectionId);

  const deleteObjectSchemaMutation = useDeleteObjectSchema(connectionId);
  const connectionQuery = connectionId
    ? `?connectionId=${encodeURIComponent(connectionId)}`
    : '';

  const handleViewInstances = useCallback(
    (objectSchemaDto: Schema) => {
      if (objectSchemaDto.name) {
        navigate(`/objects/${objectSchemaDto.name}${connectionQuery}`);
      }
    },
    [connectionQuery, navigate]
  );

  const handleEdit = useCallback(
    (objectSchemaDto: Schema) => {
      if (objectSchemaDto.id) {
        navigate(`/objects/types/${objectSchemaDto.id}${connectionQuery}`);
      }
    },
    [connectionQuery, navigate]
  );

  const handleDelete = useCallback(() => {
    if (!deleteTarget?.id) {
      return;
    }
    deleteObjectSchemaMutation.mutate(deleteTarget.id, {
      onSuccess: () => {
        toast.info('Object type has been deleted');
      },
      onSettled: () => {
        setDeleteTarget(null);
      },
    });
  }, [deleteObjectSchemaMutation, deleteTarget]);

  const deletingId = deleteObjectSchemaMutation.isPending
    ? deleteTarget?.id
    : null;

  const showSkeleton = isLoading || connectionsLoading;
  const hasContent =
    !showSkeleton && !!connectionId && !isError && objectSchemaDtos.length > 0;

  const toolbar = (
    <ConsoleToolbar
      left={<Breadcrumb items={[{ label: 'Object types' }]} />}
      actions={
        <div className="flex items-center gap-2">
          <ObjectModelConnectionSelector />
          <Can permission="database:create">
            <Button
              onClick={() =>
                navigate(`/objects/types/create${connectionQuery}`)
              }
              disabled={isError || !connectionId}
            >
              <Plus className="mr-2 h-4 w-4" />
              Create object type
            </Button>
          </Can>
        </div>
      }
    />
  );

  let body: ReactNode;
  if (showSkeleton) {
    body = (
      <TableSkeletonRows
        rows={6}
        widths={['w-40', 'w-16', 'w-48', 'ml-auto w-28']}
      />
    );
  } else if (!connectionId) {
    body = (
      <ConsoleEmptyState
        icon={
          <Icons.warning className="mb-4 h-10 w-10 text-muted-foreground" />
        }
        title="No database connection selected"
        description="Select a database connection to view its object types."
      />
    );
  } else if (isError) {
    body = <ConsoleErrorState error={error} entityLabel="object types" />;
  } else if (objectSchemaDtos.length === 0) {
    body = (
      <ConsoleEmptyState
        title="No object types yet"
        description="Create your first object type to start managing records."
      />
    );
  } else {
    body = (
      <Table variant="console">
        <TableHeader>
          <TableRow>
            <TableHead>Name</TableHead>
            <TableHead>Fields</TableHead>
            <TableHead>Description</TableHead>
            <TableHead>Updated</TableHead>
            <TableHead className="w-0" />
          </TableRow>
        </TableHeader>
        <TableBody>
          {objectSchemaDtos.map((schema) => {
            const fieldCount = schema.columns?.length ?? 0;
            return (
              <TableRow key={schema.id || schema.name}>
                <TableCell className="font-medium text-foreground">
                  {schema.name || 'Untitled object type'}
                </TableCell>
                <TableCell className="text-muted-foreground">
                  <StatusPill
                    tone="neutral"
                    dot={false}
                    label={`${fieldCount} ${fieldCount === 1 ? 'field' : 'fields'}`}
                  />
                </TableCell>
                <TableCell className="text-muted-foreground">
                  <div className="max-w-[28rem] truncate">
                    {schema.description || (
                      <span className="text-muted-foreground/60">—</span>
                    )}
                  </div>
                </TableCell>
                <TableCell className="text-muted-foreground">
                  {schema.updatedAt ? (
                    formatDate(schema.updatedAt)
                  ) : (
                    <span className="text-muted-foreground/60">—</span>
                  )}
                </TableCell>
                <TableCell className="text-right">
                  <div className="flex items-center justify-end gap-1">
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      className="text-muted-foreground"
                      title="Manage instances"
                      onClick={() => handleViewInstances(schema)}
                    >
                      <Database className="h-4 w-4" />
                    </Button>
                    <Can permission="database:update">
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        className="text-muted-foreground"
                        title="Edit object type"
                        onClick={() => handleEdit(schema)}
                      >
                        <Edit2 className="h-4 w-4" />
                      </Button>
                    </Can>
                    <Can permission="database:delete">
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        className="text-muted-foreground hover:text-destructive"
                        title="Delete object type"
                        disabled={deletingId === schema.id}
                        onClick={() => setDeleteTarget(schema)}
                      >
                        {deletingId === schema.id ? (
                          <Spinner className="h-4 w-4" />
                        ) : (
                          <Trash2 className="h-4 w-4" />
                        )}
                      </Button>
                    </Can>
                  </div>
                </TableCell>
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
    );
  }

  return (
    <>
      <ConsoleTableShell
        toolbar={toolbar}
        footer={
          hasContent ? (
            <TableStatusFooter
              left={`${objectSchemaDtos.length.toLocaleString()} object type${
                objectSchemaDtos.length === 1 ? '' : 's'
              }`}
            />
          ) : undefined
        }
      >
        {body}
      </ConsoleTableShell>

      <ModalDialog open={!!deleteTarget} onClose={() => setDeleteTarget(null)}>
        <DialogHeader>
          <DialogTitle>Delete Object Type</DialogTitle>
          <DialogDescription>
            Are you sure you want to delete the object type "
            {deleteTarget?.name}"?
          </DialogDescription>
        </DialogHeader>
        <div className="py-2">
          This action cannot be undone and may affect any workflows or records
          using this object type.
        </div>
        <DialogFooter className="gap-2 sm:gap-0">
          <DialogClose asChild>
            <Button type="button" variant="outline">
              Cancel
            </Button>
          </DialogClose>
          <Button
            type="button"
            variant="destructive"
            onClick={handleDelete}
            disabled={deleteObjectSchemaMutation.isPending}
          >
            {deleteObjectSchemaMutation.isPending
              ? 'Deleting...'
              : 'Delete Object Type'}
          </Button>
        </DialogFooter>
      </ModalDialog>
    </>
  );
}
