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
  const { selectedConnectionId, connectionQuery } =
    useObjectModelConnectionSelection();
  const { isError } = useObjectSchemaDtos(selectedConnectionId);

  return (
    <TilesPage
      kicker="Database"
      title="Object types"
      action={
        <Button
          className="h-11 w-full rounded-full sm:w-auto sm:px-6"
          onClick={() => navigate(`/objects/types/create${connectionQuery}`)}
          disabled={isError || !selectedConnectionId}
        >
          Create object type
        </Button>
      }
    >
      <div className="mb-4 flex justify-end">
        <ObjectModelConnectionSelector />
      </div>
      <TileList>
        <ObjectSchemaDtosTable connectionId={selectedConnectionId} />
      </TileList>
    </TilesPage>
  );
}
