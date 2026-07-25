import { useNavigate, useParams } from 'react-router';
import { AlertCircle } from 'lucide-react';
import {
  Alert,
  AlertDescription,
  AlertTitle,
} from '@/shared/components/ui/alert';
import { ObjectInstanceDtoForm } from '@/features/objects/components/ObjectInstanceForm';
import { useObjectSchemaDto } from '@/features/objects/hooks/useObjectSchema';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { ObjectModelConnectionSelector } from '@/features/objects/components/ObjectModelConnectionSelector';
import { useObjectModelConnectionSelection } from '@/features/objects/hooks/useObjectModelConnectionSelection';
import { Spinner } from '@/shared/components/ui/spinner';
import { PageContainer } from '@/shared/components/page-container';
import { SectionLabel } from '@/shared/components/section-label';

export function CreateObjectInstance() {
  const { typeName } = useParams<{ typeName: string }>();
  const navigate = useNavigate();
  const { selectedConnectionId, connectionQuery } =
    useObjectModelConnectionSelection();
  const { data: objectSchemaDto, isLoading } = useObjectSchemaDto(
    typeName,
    selectedConnectionId
  );

  // Set page title with object type name
  usePageTitle(
    objectSchemaDto?.name
      ? `Create ${objectSchemaDto.name} Instance`
      : 'Create Object Instance'
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
              Create {objectSchemaDto?.name ?? typeName} record
            </h1>
          </div>
        </section>

        <div className="flex justify-end px-4 sm:px-5">
          <ObjectModelConnectionSelector />
        </div>

        {isLoading ? (
          <div className="flex min-h-[40vh] items-center justify-center px-4 text-muted-foreground sm:px-5">
            <Spinner className="mr-2 size-4" />
            Loading type details...
          </div>
        ) : !objectSchemaDto ? (
          <div className="px-4 sm:px-5">
            <Alert variant="destructive">
              <AlertCircle className="size-4" />
              <AlertTitle>Object type not found</AlertTitle>
              <AlertDescription>
                The requested object type could not be loaded. Verify the URL or
                return to the objects list.
              </AlertDescription>
            </Alert>
          </div>
        ) : (
          <section className="space-y-4 px-4 sm:px-5">
            <ObjectInstanceDtoForm
              objectSchemaDto={objectSchemaDto}
              onSuccess={handleSuccess}
              connectionId={selectedConnectionId}
            />
          </section>
        )}
      </div>
    </PageContainer>
  );
}
