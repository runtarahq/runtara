import { useNavigate, useParams } from 'react-router';
import { ObjectSchemaDtoForm } from '@/features/objects/components/ObjectSchemaForm';
import {
  useObjectSchemaDtoById,
  useDeleteObjectSchema,
} from '@/features/objects/hooks/useObjectSchemas';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { toast } from 'sonner';
import { Loader2 } from 'lucide-react';

export function EditObjectSchema() {
  const { id } = useParams<{ id: string }>();
  const navigate = useNavigate();
  const { data: objectSchemaDto, isLoading } = useObjectSchemaDtoById(id);
  const deleteSchema = useDeleteObjectSchema();

  usePageTitle(
    objectSchemaDto?.name
      ? `Edit Object Type - ${objectSchemaDto.name}`
      : 'Edit Object Type'
  );

  const handleSuccess = () => {
    navigate('/objects/types');
  };

  const handleDelete = async () => {
    if (!id) return;

    const confirmed = window.confirm(
      `Are you sure you want to delete "${objectSchemaDto?.name}"? This action cannot be undone.`
    );

    if (!confirmed) return;

    try {
      await deleteSchema.mutateAsync(id);
      toast.success('Object type deleted successfully');
      navigate('/objects/types');
    } catch (error) {
      toast.error((error as Error)?.message || 'Failed to delete object type');
    }
  };

  if (isLoading) {
    return (
      <div className="min-h-screen bg-slate-50/50 dark:bg-background flex items-center justify-center">
        <Loader2 className="w-8 h-8 text-slate-400 animate-spin" />
      </div>
    );
  }

  if (!objectSchemaDto) {
    return (
      <div className="min-h-screen bg-slate-50/50 dark:bg-background flex items-center justify-center">
        <p className="text-slate-500 dark:text-slate-400">
          Object type not found
        </p>
      </div>
    );
  }

  return (
    <ObjectSchemaDtoForm
      objectSchemaDto={objectSchemaDto}
      onSuccess={handleSuccess}
      onDelete={handleDelete}
      isDeleting={deleteSchema.isPending}
    />
  );
}
