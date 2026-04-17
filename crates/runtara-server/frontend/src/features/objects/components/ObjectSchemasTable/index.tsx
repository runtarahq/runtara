import { useCallback } from 'react';
import { useNavigate } from 'react-router';
import { Schema } from '@/generated/RuntaraRuntimeApi';
import { Button } from '@/shared/components/ui/button';
import { Loader2, Edit2, Trash2, Database } from 'lucide-react';
import { Icons } from '@/shared/components/icons';
import { EntityTile } from '@/shared/components/entity-tile';
import {
  useObjectSchemaDtos,
  useDeleteObjectSchema,
} from '@/features/objects/hooks/useObjectSchemas.ts';

export function ObjectSchemaDtosTable() {
  const navigate = useNavigate();
  const {
    data: objectSchemaDtos = [],
    isLoading,
    isError,
    error,
  } = useObjectSchemaDtos();

  const deleteObjectSchemaMutation = useDeleteObjectSchema();

  const handleViewInstances = useCallback(
    (objectSchemaDto: Schema) => {
      if (objectSchemaDto.name) {
        navigate(`/objects/${objectSchemaDto.name}`);
      }
    },
    [navigate]
  );

  const handleEdit = useCallback(
    (objectSchemaDto: Schema) => {
      if (objectSchemaDto.id) {
        navigate(`/objects/types/${objectSchemaDto.id}`);
      }
    },
    [navigate]
  );

  const handleDelete = useCallback(
    (objectSchemaDto: Schema) => {
      if (objectSchemaDto.id) {
        deleteObjectSchemaMutation.mutate(objectSchemaDto.id);
      }
    },
    [deleteObjectSchemaMutation]
  );

  const renderCard = (schema: Schema) => {
    const fieldCount = schema.columns?.length || 0;
    return (
      <EntityTile
        key={schema.id || schema.name}
        title={schema.name || 'Untitled object type'}
        metadata={[
          `${fieldCount} ${fieldCount === 1 ? 'field' : 'fields'}`,
          schema.id ? `ID: ${schema.id}` : null,
        ].filter(Boolean)}
        actions={
          <>
            <Button
              variant="ghost"
              size="icon"
              className="p-2 h-auto w-auto text-slate-400 hover:text-blue-600 hover:bg-blue-50 dark:hover:bg-blue-900/30 dark:hover:text-blue-400 rounded-lg transition-colors"
              title="View instances"
              onClick={() => handleViewInstances(schema)}
            >
              <Database className="w-4 h-4" />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              className="p-2 h-auto w-auto text-slate-400 hover:text-slate-600 hover:bg-slate-100 dark:hover:bg-slate-800 dark:hover:text-slate-300 rounded-lg transition-colors"
              title="Edit object type"
              onClick={() => handleEdit(schema)}
            >
              <Edit2 className="w-4 h-4" />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              onClick={() => handleDelete(schema)}
              className="p-2 h-auto w-auto text-slate-400 hover:text-red-600 hover:bg-red-50 dark:hover:bg-red-900/30 dark:hover:text-red-400 rounded-lg transition-colors"
              title="Delete object type"
              disabled={deleteObjectSchemaMutation.isPending}
            >
              <Trash2 className="w-4 h-4" />
            </Button>
          </>
        }
      />
    );
  };

  if (isLoading) {
    return (
      <div className="space-y-3">
        {[...Array(3)].map((_, i) => (
          <div
            key={i}
            className="rounded-xl bg-muted/20 px-4 py-5 sm:px-5 sm:py-6 animate-pulse"
          >
            <div className="flex items-center gap-3">
              <div className="h-4 w-28 rounded bg-muted/60" />
            </div>
            <div className="mt-3 h-5 w-48 rounded bg-muted/60" />
          </div>
        ))}
      </div>
    );
  }

  if (isError) {
    const isNetworkError =
      (error as any)?.message?.includes('fetch') ||
      (error as any)?.code === 'ERR_NETWORK' ||
      !(error as any)?.response;

    return (
      <div className="flex flex-col items-center justify-center rounded-2xl bg-muted/20 px-6 py-12 text-center">
        <Icons.warning className="mb-4 h-12 w-12 text-destructive" />
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
          <div className="mt-4 max-w-md rounded-lg bg-destructive/10 p-3 text-left">
            <p className="text-xs font-mono text-destructive break-words">
              {(error as any).message || 'Unknown error'}
            </p>
          </div>
        )}
      </div>
    );
  }

  if (!objectSchemaDtos || objectSchemaDtos.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center rounded-2xl bg-muted/20 px-6 py-12 text-center">
        <Icons.inbox className="mb-4 h-12 w-12 text-muted-foreground" />
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
    <div className="space-y-3">
      {deleteObjectSchemaMutation.isPending && (
        <div className="flex items-center gap-2 text-xs text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" />
          Updating...
        </div>
      )}
      {objectSchemaDtos.map(renderCard)}
    </div>
  );
}
