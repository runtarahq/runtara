import { ObjectSchemaDtosTable } from '@/features/objects/components/ObjectSchemasTable';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { useObjectModelConnectionSelection } from '@/features/objects/hooks/useObjectModelConnectionSelection';

export function ObjectSchemas() {
  usePageTitle('Object Types');
  const { selectedConnectionId, isLoading: connectionsLoading } =
    useObjectModelConnectionSelection();

  return (
    <ObjectSchemaDtosTable
      connectionId={selectedConnectionId}
      connectionsLoading={connectionsLoading}
    />
  );
}
