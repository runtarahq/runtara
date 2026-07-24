import { useState } from 'react';
import { useNavigate, useParams } from 'react-router';
import { ObjectSchemaDtoForm } from '@/features/objects/components/ObjectSchemaForm';
import { ConfirmationDialog } from '@/shared/components/confirmation-dialog.tsx';
import {
  useObjectSchemaDtoById,
  useDeleteObjectSchema,
} from '@/features/objects/hooks/useObjectSchemas';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { toast } from 'sonner';
import { ObjectModelConnectionSelector } from '@/features/objects/components/ObjectModelConnectionSelector';
import { useObjectModelConnectionSelection } from '@/features/objects/hooks/useObjectModelConnectionSelection';
import { Spinner } from '@/shared/components/ui/spinner';

export function EditObjectSchema() {
  const { id } = useParams<{ id: string }>();
  const navigate = useNavigate();
  const { selectedConnectionId, connectionQuery } =
    useObjectModelConnectionSelection();
  const { data: objectSchemaDto, isLoading } = useObjectSchemaDtoById(
    id,
    selectedConnectionId
  );
  const deleteSchema = useDeleteObjectSchema(selectedConnectionId);

  usePageTitle(
    objectSchemaDto?.name
      ? `Edit Object Type - ${objectSchemaDto.name}`
      : 'Edit Object Type'
  );

  const handleSuccess = () => {
    navigate(`/objects/types${connectionQuery}`);
  };

  const [confirmDeleteOpen, setConfirmDeleteOpen] = useState(false);

  const handleDelete = () => {
    if (!id) return;
    setConfirmDeleteOpen(true);
  };

  const performDelete = async () => {
    if (!id) return;
    setConfirmDeleteOpen(false);

    try {
      await deleteSchema.mutateAsync(id);
      toast.success('Object type deleted successfully');
      navigate(`/objects/types${connectionQuery}`);
    } catch (error) {
      toast.error((error as Error)?.message || 'Failed to delete object type');
    }
  };

  if (isLoading) {
    return (
      <div className="flex min-h-screen items-center justify-center bg-muted/30 dark:bg-background">
        <Spinner className="h-8 w-8 text-muted-foreground" />
      </div>
    );
  }

  if (!objectSchemaDto) {
    return (
      <div className="flex min-h-screen items-center justify-center bg-muted/30 dark:bg-background">
        <p className="text-muted-foreground">Object type not found</p>
      </div>
    );
  }

  return (
    <div className="space-y-4">
      <div className="flex justify-end px-4 pt-4 sm:px-6 lg:px-10">
        <ObjectModelConnectionSelector />
      </div>
      <ObjectSchemaDtoForm
        objectSchemaDto={objectSchemaDto}
        onSuccess={handleSuccess}
        onDelete={handleDelete}
        isDeleting={deleteSchema.isPending}
        connectionId={selectedConnectionId}
      />
      <ConfirmationDialog
        open={confirmDeleteOpen}
        description={`Are you sure you want to delete "${objectSchemaDto?.name}"? This action cannot be undone.`}
        onClose={() => setConfirmDeleteOpen(false)}
        onConfirm={() => void performDelete()}
      />
    </div>
  );
}
