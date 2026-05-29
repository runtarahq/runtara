import { useCallback, useState } from 'react';
import { useNavigate } from 'react-router';
import { toast } from 'sonner';
import { Loader2, Edit2, Trash2, Database } from 'lucide-react';
import { Schema } from '@/generated/RuntaraRuntimeApi';
import { Button } from '@/shared/components/ui/button';
import { Badge } from '@/shared/components/ui/badge';
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/shared/components/ui/table';
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
import {
  useObjectSchemaDtos,
  useDeleteObjectSchema,
} from '@/features/objects/hooks/useObjectSchemas.ts';

interface ObjectSchemaDtosTableProps {
  connectionId?: string | null;
}

export function ObjectSchemaDtosTable({
  connectionId,
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

  if (isLoading) {
    return (
      <div className="rounded-lg border divide-y">
        {[...Array(4)].map((_, i) => (
          <div key={i} className="flex items-center gap-4 px-3 py-2.5">
            <div className="h-4 w-40 rounded bg-muted/60 animate-pulse" />
            <div className="h-4 w-16 rounded bg-muted/60 animate-pulse" />
            <div className="h-4 w-48 rounded bg-muted/60 animate-pulse" />
            <div className="ml-auto h-4 w-28 rounded bg-muted/60 animate-pulse" />
          </div>
        ))}
      </div>
    );
  }

  if (!connectionId) {
    return (
      <div className="rounded-lg border bg-muted/20 px-6 py-10 text-center">
        <Icons.warning className="mx-auto mb-4 h-10 w-10 text-muted-foreground" />
        <p className="text-base font-semibold text-foreground">
          No database connection selected
        </p>
        <p className="mt-1 text-sm text-muted-foreground">
          Select a database connection to view its object types.
        </p>
      </div>
    );
  }

  if (isError) {
    const err = error as Error & { code?: string; response?: unknown };
    const isNetworkError =
      err?.message?.includes('fetch') ||
      err?.code === 'ERR_NETWORK' ||
      !err?.response;

    return (
      <div className="rounded-lg border bg-muted/20 px-6 py-10 text-center">
        <Icons.warning className="mx-auto mb-4 h-10 w-10 text-destructive" />
        <p className="text-base font-semibold text-foreground">
          {isNetworkError
            ? 'Unable to connect to backend'
            : 'An error occurred'}
        </p>
        <p className="mt-1 text-sm text-muted-foreground">
          {isNetworkError
            ? 'Please check that the backend service is running and try again.'
            : 'There was a problem loading object types. Please try again.'}
        </p>
        {import.meta.env.DEV && error && (
          <div className="mt-4 max-w-md mx-auto rounded-lg bg-destructive/10 p-3 text-left">
            <p className="text-xs font-mono text-destructive break-words">
              {err.message || 'Unknown error'}
            </p>
          </div>
        )}
      </div>
    );
  }

  if (!objectSchemaDtos || objectSchemaDtos.length === 0) {
    return (
      <div className="rounded-lg border bg-muted/20 px-6 py-10 text-center">
        <Icons.inbox className="mx-auto mb-4 h-10 w-10 text-muted-foreground" />
        <p className="text-base font-semibold text-foreground">
          No object types yet
        </p>
        <p className="mt-1 text-sm text-muted-foreground">
          Create your first object type to start managing records.
        </p>
      </div>
    );
  }

  return (
    <>
      <div className="rounded-lg border overflow-hidden">
        <Table>
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
                    <Badge variant="secondary">
                      {fieldCount} {fieldCount === 1 ? 'field' : 'fields'}
                    </Badge>
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
                        size="icon"
                        className="h-7 w-7 text-muted-foreground"
                        title="Manage instances"
                        onClick={() => handleViewInstances(schema)}
                      >
                        <Database className="h-4 w-4" />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon"
                        className="h-7 w-7 text-muted-foreground"
                        title="Edit object type"
                        onClick={() => handleEdit(schema)}
                      >
                        <Edit2 className="h-4 w-4" />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon"
                        className="h-7 w-7 text-muted-foreground hover:text-destructive"
                        title="Delete object type"
                        disabled={deletingId === schema.id}
                        onClick={() => setDeleteTarget(schema)}
                      >
                        {deletingId === schema.id ? (
                          <Loader2 className="h-4 w-4 animate-spin" />
                        ) : (
                          <Trash2 className="h-4 w-4" />
                        )}
                      </Button>
                    </div>
                  </TableCell>
                </TableRow>
              );
            })}
          </TableBody>
        </Table>
      </div>

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
