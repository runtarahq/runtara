import { useNavigate, useParams } from 'react-router';
import { AlertCircle, Loader2 } from 'lucide-react';
import {
  Alert,
  AlertDescription,
  AlertTitle,
} from '@/shared/components/ui/alert';
import { ObjectInstanceDtoForm } from '@/features/objects/components/ObjectInstanceForm';
import { useObjectSchemaDto } from '@/features/objects/hooks/useObjectSchema';
import { usePageTitle } from '@/shared/hooks/usePageTitle';

export function CreateObjectInstance() {
  const { typeName } = useParams<{ typeName: string }>();
  const navigate = useNavigate();
  const { data: objectSchemaDto, isLoading } = useObjectSchemaDto(typeName);

  // Set page title with object type name
  usePageTitle(
    objectSchemaDto?.name
      ? `Create ${objectSchemaDto.name} Instance`
      : 'Create Object Instance'
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
              Create {objectSchemaDto?.name ?? typeName} record
            </h1>
          </div>
        </section>

        {isLoading ? (
          <div className="flex min-h-[40vh] items-center justify-center px-4 sm:px-5 text-muted-foreground">
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            Loading type details...
          </div>
        ) : !objectSchemaDto ? (
          <div className="px-4 sm:px-5">
            <Alert variant="destructive">
              <AlertCircle className="h-4 w-4" />
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
            />
          </section>
        )}
      </div>
    </div>
  );
}
