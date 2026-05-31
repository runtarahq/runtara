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
import { useObjectModelConnectionSelection } from '../../hooks/useObjectModelConnectionSelection';

export function ManageInstances() {
  const { typeName } = useParams<{ typeName: string }>();
  const { selectedConnectionId } = useObjectModelConnectionSelection();
  const { data: objectSchemaDto, isLoading } = useObjectSchemaDto(
    typeName,
    selectedConnectionId
  );

  // Set page title with object type name
  usePageTitle(
    objectSchemaDto?.name
      ? `Object Instances - ${objectSchemaDto.name}`
      : 'Object Instances'
  );

  if (isLoading) {
    return (
      <div className="flex h-dvh items-center justify-center text-muted-foreground">
        <Loader2 className="mr-2 h-4 w-4 animate-spin" />
        Loading records...
      </div>
    );
  }

  if (!objectSchemaDto) {
    return (
      <div className="flex h-dvh items-center justify-center p-6">
        <Alert variant="destructive" className="max-w-2xl">
          <AlertCircle className="h-4 w-4" />
          <AlertTitle>Object type not found</AlertTitle>
          <AlertDescription>
            The requested object type could not be loaded. Verify the URL or
            return to the objects list.
          </AlertDescription>
        </Alert>
      </div>
    );
  }

  return (
    <ObjectInstanceDtosTable
      objectSchemaDto={objectSchemaDto}
      connectionId={selectedConnectionId}
    />
  );
}
