import { useNavigate, useParams } from 'react-router';
import { ObjectInstanceDtoForm } from '@/features/objects/components/ObjectInstanceForm';
import { useObjectSchemaDto } from '@/features/objects/hooks/useObjectSchema';
import { useObjectInstanceDto } from '@/features/objects/hooks/useObjectRecords';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { ObjectModelConnectionSelector } from '@/features/objects/components/ObjectModelConnectionSelector';
import { useObjectModelConnectionSelection } from '@/features/objects/hooks/useObjectModelConnectionSelection';
import { Spinner } from '@/shared/components/ui/spinner';
import { PageContainer } from '@/shared/components/page-container';
import { SectionLabel } from '@/shared/components/section-label';

export function EditObjectInstance() {
  const { typeName, id } = useParams<{ typeName: string; id: string }>();
  const navigate = useNavigate();
  const { selectedConnectionId, connectionQuery } =
    useObjectModelConnectionSelection();
  const { data: objectSchemaDto, isLoading: isSchemaLoading } =
    useObjectSchemaDto(typeName, selectedConnectionId);
  const { data: record, isLoading: isRecordLoading } = useObjectInstanceDto(
    objectSchemaDto?.id ?? undefined,
    id,
    selectedConnectionId
  );

  // Set page title with object type name
  usePageTitle(
    objectSchemaDto?.name
      ? `Edit ${objectSchemaDto.name} Instance`
      : 'Edit Object Instance'
  );

  const handleSuccess = () => {
    navigate(`/objects/${typeName}${connectionQuery}`);
  };

  return (
    <PageContainer>
      <div className="mx-auto flex w-full max-w-5xl flex-col gap-6">
        <section className="rounded-lg bg-transparent px-4 py-4 sm:px-5">
          <div className="space-y-2">
            <SectionLabel>Objects</SectionLabel>
            <h1 className="text-3xl font-semibold leading-tight text-foreground">
              Edit {objectSchemaDto?.name ?? typeName} record
            </h1>
          </div>
        </section>

        <div className="flex justify-end px-4 sm:px-5">
          <ObjectModelConnectionSelector />
        </div>

        {isSchemaLoading || isRecordLoading ? (
          <div className="flex min-h-[40vh] items-center justify-center px-4 text-muted-foreground sm:px-5">
            <Spinner className="mr-2 h-4 w-4" />
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
              connectionId={selectedConnectionId}
            />
          </section>
        )}
      </div>
    </PageContainer>
  );
}
