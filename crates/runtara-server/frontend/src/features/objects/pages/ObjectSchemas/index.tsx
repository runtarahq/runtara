import { useNavigate } from 'react-router';
import { Button } from '@/shared/components/ui/button';
import { ObjectSchemaDtosTable } from '@/features/objects/components/ObjectSchemasTable';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { TilesPage, TileList } from '@/shared/components/tiles-page';
import { useObjectSchemaDtos } from '@/features/objects/hooks/useObjectSchemas';
import { ObjectModelConnectionSelector } from '@/features/objects/components/ObjectModelConnectionSelector';
import { useObjectModelConnectionSelection } from '@/features/objects/hooks/useObjectModelConnectionSelection';

export function ObjectSchemas() {
  const navigate = useNavigate();
  usePageTitle('Object Types');
  const {
    selectedConnectionId,
    connectionQuery,
    isLoading: connectionsLoading,
  } = useObjectModelConnectionSelection();
  const { isError } = useObjectSchemaDtos(selectedConnectionId);

  return (
    <TilesPage
      kicker="Database"
      title="Object types"
      action={
        <Button
          className="w-full sm:w-auto sm:px-4"
          onClick={() => navigate(`/objects/types/create${connectionQuery}`)}
          disabled={isError || !selectedConnectionId}
        >
          Create object type
        </Button>
      }
      toolbar={<ObjectModelConnectionSelector />}
    >
      <TileList>
        <ObjectSchemaDtosTable
          connectionId={selectedConnectionId}
          connectionsLoading={connectionsLoading}
        />
      </TileList>
    </TilesPage>
  );
}
