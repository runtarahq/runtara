import { useNavigate, useParams } from 'react-router';
import { ObjectInstanceDtoForm } from '@/features/objects/components/ObjectInstanceForm';
import { useObjectSchemaDto } from '@/features/objects/hooks/useObjectSchema';
import { useObjectInstanceDto } from '@/features/objects/hooks/useObjectRecords';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { Loader2 } from 'lucide-react';

export function EditObjectInstance() {
  const { typeName, id } = useParams<{ typeName: string; id: string }>();
  const navigate = useNavigate();
  const { data: objectSchemaDto, isLoading: isSchemaLoading } =
    useObjectSchemaDto(typeName);
  const { data: record, isLoading: isRecordLoading } = useObjectInstanceDto(
    objectSchemaDto?.id ?? undefined,
    id
  );

  // Set page title with object type name
  usePageTitle(
    objectSchemaDto?.name
      ? `Edit ${objectSchemaDto.name} Instance`
      : 'Edit Object Instance'
  );

  const handleSuccess = () => {
    navigate(`/objects/${typeName}`);
  };

  return (
    <div className="w-full px-4 py-6 sm:px-6 lg:px-10">
      <div className="mx-auto flex w-full max-w-5xl flex-col gap-6">
        <section className="rounded-2xl bg-transparent px-4 py-4 sm:px-5">
          <div className="space-y-2">
            <p className="text-xs font-semibold uppercase tracking-[0.08em] text-muted-foreground">
              Objects
            </p>
            <h1 className="text-3xl font-semibold leading-tight text-slate-900/90">
              Edit {objectSchemaDto?.name ?? typeName} record
            </h1>
          </div>
        </section>

        {isSchemaLoading || isRecordLoading ? (
          <div className="flex min-h-[40vh] items-center justify-center px-4 sm:px-5 text-muted-foreground">
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            Loading data...
          </div>
        ) : !objectSchemaDto ? (
          <div className="px-4 sm:px-5">Object type not found</div>
        ) : !record ? (
          <div className="px-4 sm:px-5">Record not found</div>
        ) : (
          <section className="space-y-4 px-4 sm:px-5">
            <ObjectInstanceDtoForm
              objectSchemaDto={objectSchemaDto}
              record={record}
              onSuccess={handleSuccess}
            />
          </section>
        )}
      </div>
    </div>
  );
}
