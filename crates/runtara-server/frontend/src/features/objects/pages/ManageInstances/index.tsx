import { useParams } from 'react-router';
import { AlertCircle, Loader2 } from 'lucide-react';
import {
  Alert,
  AlertDescription,
  AlertTitle,
} from '@/shared/components/ui/alert';
import { ObjectInstanceDtosTable } from '../../components/ObjectInstancesTable';
import { useObjectSchemaDto } from '../../hooks/useObjectSchema';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { TileList, TilesPage } from '@/shared/components/tiles-page';

export function ManageInstances() {
  const { typeName } = useParams<{ typeName: string }>();
  const { data: objectSchemaDto, isLoading } = useObjectSchemaDto(typeName);

  // Set page title with object type name
  usePageTitle(
    objectSchemaDto?.name
      ? `Object Instances - ${objectSchemaDto.name}`
      : 'Object Instances'
  );

  return (
    <TilesPage
      kicker="Objects"
      title={`Manage ${objectSchemaDto?.name ?? typeName}`}
      className="py-6 lg:px-10"
      contentClassName="gap-6"
    >
      {isLoading ? (
        <div className="flex min-h-[40vh] items-center justify-center text-muted-foreground">
          <Loader2 className="mr-2 h-4 w-4 animate-spin" />
          Loading records...
        </div>
      ) : !objectSchemaDto ? (
        <div className="max-w-3xl">
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
        <TileList>
          <ObjectInstanceDtosTable objectSchemaDto={objectSchemaDto} />
        </TileList>
      )}
    </TilesPage>
  );
}
